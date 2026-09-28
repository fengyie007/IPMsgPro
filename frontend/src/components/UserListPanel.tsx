import React, { useState, useMemo } from 'react';
import { FiSearch, FiRefreshCw, FiMessageSquare, FiUsers, FiChevronDown, FiChevronRight } from 'react-icons/fi';
import { useUserStore } from '../stores/userStore';
import { useMessageStore } from '../stores/messageStore';
import { useConfigStore } from '../stores/configStore';
import { Message, User } from '../types';
import { formatListTime, formatPreview } from '../utils/format';
import { ViewMode } from './LeftSidebar';

interface UserListPanelProps {
  viewMode: ViewMode;
  onViewChange?: (mode: ViewMode) => void;
}

/** Contacts sharing one group name. key is the trimmed group name, '' for no group. */
interface ContactGroup {
  key: string;
  users: User[];
  online: number;
}

const UNGROUPED_LABEL = '未分组';
const STATUS_ORDER: Record<User['status'], number> = { online: 0, away: 1, offline: 2 };
// Pinyin order for Chinese names; one shared collator is much cheaper than localeCompare per call
const collator = new Intl.Collator('zh-CN');

/**
 * Group contacts by group name: the local user's own group first, then the
 * other groups by name, contacts without a group last. Inside a group:
 * online, away, offline, then by nickname.
 */
function groupContacts(users: User[], ownGroup: string): ContactGroup[] {
  const byKey = new Map<string, User[]>();
  for (const user of users) {
    const key = (user.group || '').trim();
    const members = byKey.get(key);
    if (members) {
      members.push(user);
    } else {
      byKey.set(key, [user]);
    }
  }

  const own = ownGroup.trim();
  const rank = (key: string) => (key === '' ? 2 : key === own ? 0 : 1);
  return Array.from(byKey, ([key, members]) => ({
    key,
    users: members.sort((a, b) =>
      (STATUS_ORDER[a.status] ?? 2) - (STATUS_ORDER[b.status] ?? 2) ||
      collator.compare(a.nickname, b.nickname)),
    online: members.filter((u) => u.status !== 'offline').length,
  })).sort((a, b) => rank(a.key) - rank(b.key) || collator.compare(a.key, b.key));
}

export default function UserListPanel({ viewMode, onViewChange }: UserListPanelProps) {
  const [searchText, setSearchText] = useState('');
  // Collapsed contact groups (ContactGroup keys). Kept for this run only; the
  // panel stays mounted when the view changes, so switching views keeps them.
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  const ownGroup = useConfigStore((s) => s.config.group);
  const users = useUserStore((s) => s.users);
  const currentUser = useUserStore((s) => s.currentUser);
  const setCurrentUser = useUserStore((s) => s.setCurrentUser);
  const discoverUsers = useUserStore((s) => s.discoverUsers);
  const loading = useUserStore((s) => s.loading);
  const messages = useMessageStore((s) => s.messages);
  const unread = useMessageStore((s) => s.unread);

  // Get conversation users - users with any messages, sorted by latest message
  const conversationUsers = useMemo(() => {
    const userLastMsg: { user: User; lastMsg: Message }[] = [];

    // First, add users from userStore who have messages
    for (const user of users) {
      const userMsgs = messages.get(user.id);
      if (userMsgs && userMsgs.length > 0) {
        const lastMsg = userMsgs[userMsgs.length - 1];
        userLastMsg.push({ user, lastMsg });
      }
    }

    // Also add users who have messages but aren't in userStore (from history)
    for (const [partnerId, userMsgs] of messages) {
      if (userMsgs && userMsgs.length > 0 && !users.some(u => u.id === partnerId)) {
        const lastMsg = userMsgs[userMsgs.length - 1];
        // Create a minimal user object from the partner ID
        const [username, hostname] = partnerId.split('@');
        const virtualUser: User = {
          id: partnerId,
          nickname: username || partnerId,
          username: username || '',
          hostname: hostname || '',
          group: '',
          ip: '',
          port: 0,
          status: 'offline',
          version: '',
        };
        userLastMsg.push({ user: virtualUser, lastMsg });
      }
    }

    // Sort by latest message timestamp (newest first)
    userLastMsg.sort((a, b) => b.lastMsg.timestamp - a.lastMsg.timestamp);
    return userLastMsg;
  }, [users, messages]);

  // Contacts filtered by search text, then grouped. The search also matches the
  // group name, so typing a group name lists all of its members.
  const contactGroups = useMemo(() => {
    const query = searchText.toLowerCase();
    const matched = users.filter((u) =>
      u.nickname.toLowerCase().includes(query) ||
      u.username.toLowerCase().includes(query) ||
      u.ip.includes(searchText) ||
      (u.group || '').toLowerCase().includes(query)
    );
    return groupContacts(matched, ownGroup);
  }, [users, searchText, ownGroup]);

  const filteredConversations = conversationUsers.filter(({ user }) =>
    user.nickname.toLowerCase().includes(searchText.toLowerCase()) ||
    user.username.toLowerCase().includes(searchText.toLowerCase()) ||
    (user.ip && user.ip.includes(searchText))
  );

  const handleRefresh = async () => {
    await discoverUsers();
  };

  const toggleGroup = (key: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  };

  const handleSelectUser = (user: User) => {
    setCurrentUser(user);
    // If we're in contacts mode, switch to chat mode after selecting a user
    if (viewMode === 'contacts' && onViewChange) {
      onViewChange('chat');
    }
  };

  const isContactsMode = viewMode === 'contacts';
  const searching = searchText !== '';

  return (
    <div className="w-user-list bg-list-bg border-r border-gray-200 flex flex-col shrink-0">
      {/* Search bar */}
      <div className="p-3 flex items-center gap-2">
        <div className="flex-1 relative">
          <FiSearch className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" size={14} />
          <input
            type="text"
            placeholder={isContactsMode ? '搜索联系人' : '搜索对话'}
            value={searchText}
            onChange={(e) => setSearchText(e.target.value)}
            className="w-full pl-8 pr-3 py-1.5 text-sm bg-gray-200/60 rounded-md
                       focus:outline-none focus:ring-1 focus:ring-primary-400
                       placeholder-gray-400"
          />
        </div>
        <button
          className={`p-1.5 rounded-md hover:bg-gray-200 text-gray-500 transition-colors
            ${loading ? 'animate-spin' : ''}`}
          onClick={handleRefresh}
          title="刷新用户列表"
        >
          <FiRefreshCw size={16} />
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto">
        {isContactsMode ? (
          // Contacts mode - all users, grouped by group name
          contactGroups.length === 0 ? (
            <div className="text-center text-gray-400 py-10">
              <FiUsers size={32} className="mx-auto mb-2 opacity-50" />
              <p className="text-sm">暂无在线用户</p>
              <p className="text-xs mt-1">点击刷新按钮搜索</p>
            </div>
          ) : (
            contactGroups.map((group) => {
              // A search shows every match, whatever the collapsed state
              const open = searching || !collapsedGroups.has(group.key);
              return (
                <section key={`group:${group.key}`}>
                  <GroupHeader
                    label={group.key || UNGROUPED_LABEL}
                    online={group.online}
                    total={group.users.length}
                    open={open}
                    disabled={searching}
                    onToggle={() => toggleGroup(group.key)}
                  />
                  {open && group.users.map((user) => (
                    <ContactCard
                      key={user.id}
                      user={user}
                      selected={currentUser?.id === user.id}
                      onClick={() => handleSelectUser(user)}
                    />
                  ))}
                </section>
              );
            })
          )
        ) : (
          // Chat mode - show conversations (recent 7 days)
          filteredConversations.length === 0 ? (
            <div className="text-center text-gray-400 py-10">
              <FiMessageSquare size={32} className="mx-auto mb-2 opacity-50" />
              <p className="text-sm">暂无对话</p>
              <p className="text-xs mt-1">在通讯录中选择用户开始聊天</p>
            </div>
          ) : (
            filteredConversations.map(({ user, lastMsg }) => (
              <ConversationCard
                key={user.id}
                user={user}
                selected={currentUser?.id === user.id}
                lastMessage={lastMsg}
                unread={unread.get(user.id) ?? 0}
                onClick={() => handleSelectUser(user)}
              />
            ))
          )
        )}
      </div>
    </div>
  );
}

// Conversation card - shows user with last message preview and unread badge
function ConversationCard({ user, selected, lastMessage, unread, onClick }: {
  user: User;
  selected: boolean;
  lastMessage: Message;
  unread: number;
  onClick: () => void;
}) {
  const statusColor = user.status === 'online'
    ? 'bg-green-500'
    : user.status === 'away'
    ? 'bg-yellow-500 status-away'
    : 'bg-gray-400';

  return (
    <div
      className={`flex items-center gap-3 px-3 py-3 cursor-pointer transition-colors
        ${selected ? 'bg-gray-200/80' : 'hover:bg-gray-200/50'}`}
      onClick={onClick}
    >
      <div className="relative shrink-0">
        <div className="w-10 h-10 rounded-lg bg-primary-100 flex items-center justify-center">
          <span className="text-primary-600 font-medium text-sm">
            {user.nickname.charAt(0).toUpperCase()}
          </span>
        </div>
        <div className={`absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full border-2 border-list-bg ${statusColor}`} />
      </div>
      <div className="flex-1 min-w-0">
        <div className="flex items-center justify-between gap-2">
          <span className={`text-sm truncate ${unread > 0 ? 'font-semibold text-gray-900' : 'font-medium text-gray-800'}`}>
            {user.nickname}
          </span>
          <span className="text-[11px] text-gray-400 shrink-0">{formatListTime(lastMessage.timestamp)}</span>
        </div>
        <div className="flex items-center justify-between gap-2 mt-0.5">
          <p className="text-xs text-gray-400 truncate">
            {formatPreview(lastMessage)}
          </p>
          {unread > 0 && (
            <span
              className="shrink-0 min-w-[18px] h-[18px] px-1 rounded-full bg-red-500 text-white text-[10px] leading-[18px] text-center"
              title={`${unread} 条未读`}
            >
              {unread > 99 ? '99+' : unread}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

// Group header in contacts mode - click to collapse / expand the group. Sticky
// inside its <section>, so it stays on top while that group is scrolled.
function GroupHeader({ label, online, total, open, disabled, onToggle }: {
  label: string;
  online: number;
  total: number;
  open: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      className={`sticky top-0 z-10 w-full flex items-center gap-1 px-3 py-1.5 bg-list-bg
        text-xs text-gray-500 transition-colors
        ${disabled ? 'cursor-default' : 'hover:text-gray-800'}`}
      onClick={onToggle}
      disabled={disabled}
      aria-expanded={open}
    >
      {open ? <FiChevronDown size={14} className="shrink-0" /> : <FiChevronRight size={14} className="shrink-0" />}
      <span className="truncate">{label}</span>
      <span className="ml-auto pl-2 shrink-0 text-gray-400" title={`${online} 人在线，共 ${total} 人`}>
        {online}/{total}
      </span>
    </button>
  );
}

// Contact card - one user inside a contacts group
function ContactCard({ user, selected, onClick }: {
  user: User;
  selected: boolean;
  onClick: () => void;
}) {
  const statusColor = user.status === 'online'
    ? 'bg-green-500'
    : user.status === 'away'
    ? 'bg-yellow-500 status-away'
    : 'bg-gray-400';

  return (
    <div
      className={`flex items-center gap-3 px-3 py-3 cursor-pointer transition-colors
        ${selected ? 'bg-gray-200/80' : 'hover:bg-gray-200/50'}`}
      onClick={onClick}
    >
      <div className="relative shrink-0">
        <div className="w-10 h-10 rounded-lg bg-primary-100 flex items-center justify-center">
          <span className="text-primary-600 font-medium text-sm">
            {user.nickname.charAt(0).toUpperCase()}
          </span>
        </div>
        <div className={`absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full border-2 border-list-bg ${statusColor}`} />
      </div>
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium text-gray-800 truncate">
          {user.nickname}
        </p>
        <p className="text-xs text-gray-400 truncate mt-0.5">
          {user.ip}:{user.port}
        </p>
      </div>
    </div>
  );
}
