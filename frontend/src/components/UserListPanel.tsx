import React, { useState, useMemo } from 'react';
import { FiSearch, FiRefreshCw, FiMessageSquare, FiUsers } from 'react-icons/fi';
import { useUserStore } from '../stores/userStore';
import { useMessageStore } from '../stores/messageStore';
import { User } from '../types';
import { ViewMode } from './LeftSidebar';

interface UserListPanelProps {
  viewMode: ViewMode;
  onViewChange?: (mode: ViewMode) => void;
}

export default function UserListPanel({ viewMode, onViewChange }: UserListPanelProps) {
  const [searchText, setSearchText] = useState('');
  const users = useUserStore((s) => s.users);
  const currentUser = useUserStore((s) => s.currentUser);
  const setCurrentUser = useUserStore((s) => s.setCurrentUser);
  const discoverUsers = useUserStore((s) => s.discoverUsers);
  const loading = useUserStore((s) => s.loading);
  const messages = useMessageStore((s) => s.messages);
  const unread = useMessageStore((s) => s.unread);

  // Get conversation users - users with any messages, sorted by latest message
  const conversationUsers = useMemo(() => {
    const userLastMsg: { user: User; lastTimestamp: number; lastContent: string }[] = [];

    // First, add users from userStore who have messages
    for (const user of users) {
      const userMsgs = messages.get(user.id);
      if (userMsgs && userMsgs.length > 0) {
        const lastMsg = userMsgs[userMsgs.length - 1];
        userLastMsg.push({
          user,
          lastTimestamp: lastMsg.timestamp,
          lastContent: lastMsg.content,
        });
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
        userLastMsg.push({
          user: virtualUser,
          lastTimestamp: lastMsg.timestamp,
          lastContent: lastMsg.content,
        });
      }
    }

    // Sort by latest message timestamp (newest first)
    userLastMsg.sort((a, b) => b.lastTimestamp - a.lastTimestamp);
    return userLastMsg;
  }, [users, messages]);

  // Filter by search text
  const filteredUsers = users.filter((u) =>
    u.nickname.toLowerCase().includes(searchText.toLowerCase()) ||
    u.username.toLowerCase().includes(searchText.toLowerCase()) ||
    u.ip.includes(searchText)
  );

  const filteredConversations = conversationUsers.filter(({ user }) =>
    user.nickname.toLowerCase().includes(searchText.toLowerCase()) ||
    user.username.toLowerCase().includes(searchText.toLowerCase()) ||
    (user.ip && user.ip.includes(searchText))
  );

  const handleRefresh = async () => {
    await discoverUsers();
  };

  const handleSelectUser = (user: User) => {
    setCurrentUser(user);
    // If we're in contacts mode, switch to chat mode after selecting a user
    if (viewMode === 'contacts' && onViewChange) {
      onViewChange('chat');
    }
  };

  const isContactsMode = viewMode === 'contacts';

  return (
    <div className="w-[300px] bg-[#F7F7F7] border-r border-gray-200 flex flex-col shrink-0">
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
          // Contacts mode - show all users
          filteredUsers.length === 0 ? (
            <div className="text-center text-gray-400 py-10">
              <FiUsers size={32} className="mx-auto mb-2 opacity-50" />
              <p className="text-sm">暂无在线用户</p>
              <p className="text-xs mt-1">点击刷新按钮搜索</p>
            </div>
          ) : (
            filteredUsers.map((user) => (
              <ContactCard
                key={user.id}
                user={user}
                selected={currentUser?.id === user.id}
                onClick={() => handleSelectUser(user)}
              />
            ))
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
            filteredConversations.map(({ user, lastContent }) => (
              <ConversationCard
                key={user.id}
                user={user}
                selected={currentUser?.id === user.id}
                lastMessage={lastContent}
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
  lastMessage: string;
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
        <div className={`absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full border-2 border-[#F7F7F7] ${statusColor}`} />
      </div>
      <div className="flex-1 min-w-0">
        <div className="flex items-center justify-between gap-2">
          <span className={`text-sm truncate ${unread > 0 ? 'font-semibold text-gray-900' : 'font-medium text-gray-800'}`}>
            {user.nickname}
          </span>
          <span className="flex items-center gap-2 shrink-0">
            {user.group && (
              <span className="text-xs text-gray-400">{user.group}</span>
            )}
            <span className="text-[10px] text-gray-400">{user.ip}</span>
          </span>
        </div>
        <div className="flex items-center justify-between gap-2 mt-0.5">
          <p className="text-xs text-gray-400 truncate">
            {lastMessage || `${user.ip}:${user.port}`}
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

// Contact card - shows all users
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
        <div className={`absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full border-2 border-[#F7F7F7] ${statusColor}`} />
      </div>
      <div className="flex-1 min-w-0">
        <div className="flex items-center justify-between gap-2">
          <span className="text-sm font-medium text-gray-800 truncate">
            {user.nickname}
          </span>
          {user.group && (
            <span className="text-xs text-gray-400 shrink-0">{user.group}</span>
          )}
        </div>
        <p className="text-xs text-gray-400 truncate mt-0.5">
          {user.ip}:{user.port}
        </p>
      </div>
    </div>
  );
}
