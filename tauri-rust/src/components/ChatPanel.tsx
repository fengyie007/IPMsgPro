import React, { useEffect, useRef, useState } from 'react';
import { FiCamera, FiImage, FiFile, FiSmile, FiMoreHorizontal, FiTrash2, FiChevronUp, FiSearch, FiX } from 'react-icons/fi';
import { useUserStore } from '../stores/userStore';
import { useMessageStore } from '../stores/messageStore';
import { toast } from '../stores/toastStore';
import { buildEmojiMessage, emojiStyle } from '../emojiData';
import { isSameDay, formatDateSeparator } from '../utils/format';
import type { Message } from '../types';
import ConfirmDialog from './ConfirmDialog';
import MessageBubble from './MessageBubble';
import EmojiPicker from './EmojiPicker';

const BLOCK_TAG = /^(DIV|P|LI|TR|PRE|BLOCKQUOTE|H[1-6])$/;
function serializeEditor(root: HTMLElement): string {
  let out = '';
  const lineBreak = () => { if (out && !out.endsWith('\n')) out += '\n'; };
  const walk = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) { out += node.textContent || ''; return; }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const elem = node as HTMLElement;
    if (elem.dataset.emojiId) { out += buildEmojiMessage(elem.dataset.emojiId); return; }
    if (elem.tagName === 'BR') { out += '\n'; return; }
    const block = BLOCK_TAG.test(elem.tagName);
    if (block) lineBreak();
    elem.childNodes.forEach(walk);
    if (block) lineBreak();
  };
  root.childNodes.forEach(walk);
  return out;
}

export default function ChatPanel() {
  const user = useUserStore((s) => s.currentUser);
  const userId = user?.id || '';
  const userMessages = useMessageStore((s) => s.messages.get(userId)) || [];
  const page = useMessageStore((s) => s.historyPages.get(userId));
  const loading = useMessageStore((s) => s.loading);
  const [hasInput, setHasInput] = useState(false);
  const [sending, setSending] = useState(false);
  const sendingRef = useRef(false);
  const editorRef = useRef<HTMLDivElement>(null);
  const savedRange = useRef<Range | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const nearBottom = useRef(true);
  const prependHeight = useRef<number | null>(null);
  const [emojiOpen, setEmojiOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [clearOpen, setClearOpen] = useState(false);
  const clearing = useRef(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [searchResults, setSearchResults] = useState<Message[] | null>(null);
  const [searching, setSearching] = useState(false);
  const searchGeneration = useRef(0);
  const visibleMessages = searchResults ?? userMessages;

  useEffect(() => {
    if (userId) {
      void useMessageStore.getState().loadHistory(userId);
      useMessageStore.getState().clearUnread(userId);
    }
    return () => { ++searchGeneration.current; };
  }, [userId]);
  const lastMessage = visibleMessages[visibleMessages.length - 1];
  const scrollKey = `${lastMessage?.id || ''}|${visibleMessages.length}|${searchResults !== null}`;
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    if (prependHeight.current !== null) {
      el.scrollTop += el.scrollHeight - prependHeight.current;
      prependHeight.current = null;
    } else if (nearBottom.current || lastMessage?.from === 'self') el.scrollTop = el.scrollHeight;
  }, [scrollKey]);

  const syncHasInput = () => {
    const el = editorRef.current;
    setHasInput(!!el && (!!(el.textContent || '').trim() || !!el.querySelector('[data-emoji-id]')));
  };
  const saveSelection = () => {
    const selection = window.getSelection();
    if (selection?.rangeCount) {
      const range = selection.getRangeAt(0);
      if (editorRef.current?.contains(range.commonAncestorContainer)) savedRange.current = range;
    }
  };
  const insertEmoji = (id: string) => {
    const el = editorRef.current;
    if (!el || sendingRef.current) return;
    el.focus();
    const selection = window.getSelection();
    let range = savedRange.current;
    if (!range || !el.contains(range.commonAncestorContainer)) {
      range = document.createRange(); range.selectNodeContents(el); range.collapse(false);
    }
    range.deleteContents();
    const span = document.createElement('span');
    span.dataset.emojiId = id; span.contentEditable = 'false';
    Object.assign(span.style, emojiStyle(id, 18));
    range.insertNode(span);
    const after = document.createRange(); after.setStartAfter(span); after.collapse(true);
    selection?.removeAllRanges(); selection?.addRange(after); savedRange.current = after;
    syncHasInput();
  };
  const send = async () => {
    if (!user || !editorRef.current || sendingRef.current) return;
    const content = serializeEditor(editorRef.current).replace(/\s+$/g, '');
    if (!content.trim()) return;
    sendingRef.current = true; setSending(true);
    try {
      const ok = await useMessageStore.getState().sendMessage(user.id, content);
      if (!ok) { toast.error('发送失败：' + (useMessageStore.getState().error || '后端未接受消息')); return; }
      if (editorRef.current) editorRef.current.innerHTML = '';
      savedRange.current = null; setHasInput(false); setSearchResults(null); setQuery('');
    } finally { sendingRef.current = false; setSending(false); }
  };
  const handleKeyDown = (event: React.KeyboardEvent) => {
    if (event.key !== 'Enter' || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (sendingRef.current) return;
    if (event.shiftKey || event.ctrlKey) { document.execCommand('insertLineBreak'); syncHasInput(); }
    else void send();
  };
  const pasteText = (event: React.ClipboardEvent) => {
    event.preventDefault();
    if (sendingRef.current) return;
    const text = event.clipboardData.getData('text/plain');
    if (text) { document.execCommand('insertText', false, text); syncHasInput(); saveSelection(); }
    else if (event.clipboardData.files.length || event.clipboardData.getData('text/html')) {
      toast.info('Rust 核心版暂不支持粘贴图片或富文本，仅支持纯文本和内置表情');
    }
  };
  const loadMore = async () => {
    const count = userMessages.length;
    prependHeight.current = listRef.current?.scrollHeight ?? null;
    await useMessageStore.getState().loadMoreHistory(userId);
    if ((useMessageStore.getState().messages.get(userId)?.length || 0) === count) prependHeight.current = null;
  };
  const clear = async () => {
    if (clearing.current) return;
    clearing.current = true;
    try {
      await useMessageStore.getState().clearHistory(userId);
      setClearOpen(false); setSearchResults(null); setQuery('');
      toast.success('聊天记录已清空');
    } catch (error) { toast.error('清空失败：' + String(error)); }
    finally { clearing.current = false; }
  };
  const search = async () => {
    const generation = ++searchGeneration.current;
    if (!query.trim()) { setSearchResults(null); setSearching(false); return; }
    setSearching(true);
    try {
      const messages = await useMessageStore.getState().searchMessages(query, userId);
      if (generation === searchGeneration.current) setSearchResults(messages);
    } catch (error) {
      if (generation === searchGeneration.current) toast.error('搜索失败：' + String(error));
    } finally { if (generation === searchGeneration.current) setSearching(false); }
  };
  const closeSearch = () => {
    ++searchGeneration.current; setSearchOpen(false); setQuery(''); setSearchResults(null); setSearching(false);
  };
  if (!user) return null;
  const status = user.status === 'online' ? '在线' : user.status === 'away' ? '离开' : '离线';
  return (
    <div className="flex-1 flex flex-col min-w-0 bg-white">
      <div className="h-16 px-5 flex items-center border-b border-gray-200 shrink-0 gap-2">
        <div className="min-w-0"><h2 className="font-semibold text-gray-800 truncate">{user.nickname}</h2><p className="text-xs text-gray-400 mt-0.5 truncate">{status}{user.group ? ` · ${user.group}` : ''} · {user.ip ? `${user.ip}:${user.port}` : '历史联系人'}</p></div>
        <button className="ml-auto p-2 text-gray-500 rounded hover:bg-gray-100" title="搜索本会话历史" onClick={() => setSearchOpen(true)}><FiSearch size={17} /></button>
        <div className="relative">
          <button type="button" title="更多" onClick={() => setMenuOpen(!menuOpen)} className="p-2 text-gray-500 rounded hover:bg-gray-100"><FiMoreHorizontal size={18} /></button>
          {menuOpen && <><div className="fixed inset-0 z-10" onClick={() => setMenuOpen(false)} /><div className="absolute right-0 mt-1 w-40 bg-white rounded shadow-lg border py-1 z-20"><button onClick={() => { setMenuOpen(false); setClearOpen(true); }} className="w-full flex items-center gap-2 px-3 py-2 text-sm text-red-600 hover:bg-gray-100"><FiTrash2 size={14} />清空聊天记录</button></div></>}
        </div>
      </div>
      {searchOpen && <div className="flex items-center gap-2 px-4 py-2 border-b bg-gray-50">
        <input autoFocus className="input-field flex-1" value={query} placeholder="搜索当前会话历史" onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); void search(); } }} />
        <button disabled={searching} onClick={search} className="text-sm text-primary-600 disabled:opacity-50">{searching ? '搜索中…' : '搜索'}</button>
        <button onClick={closeSearch} className="p-1 text-gray-500" title="返回聊天"><FiX /></button>
      </div>}
      <div ref={listRef} onScroll={() => { const el = listRef.current; if (el) nearBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40; }} className="flex-1 overflow-y-auto p-4 space-y-3 bg-chat-bg">
        {searchResults !== null && <p className="text-center text-xs text-gray-400">搜索结果：{searchResults.length} 条</p>}
        {page?.hasMore && searchResults === null && <div className="flex justify-center"><button disabled={loading} onClick={loadMore} className="flex items-center gap-1 px-3 py-1 text-xs text-gray-500 bg-white/70 rounded-full disabled:opacity-50"><FiChevronUp size={12} />{loading ? '加载中…' : '加载更早的消息'}</button></div>}
        {!visibleMessages.length && <p className="text-center text-gray-400 text-sm mt-10">{searchResults !== null ? '没有匹配的历史消息' : loading ? '正在加载历史…' : '暂无消息，发送一条消息开始聊天'}</p>}
        {visibleMessages.map((message, index) => <React.Fragment key={message.id}>
          {(!visibleMessages[index - 1] || !isSameDay(visibleMessages[index - 1].timestamp, message.timestamp)) && <div className="flex justify-center"><span className="text-[11px] text-gray-500 bg-gray-200/70 px-2 py-0.5 rounded">{formatDateSeparator(message.timestamp)}</span></div>}
          <MessageBubble message={message} />
        </React.Fragment>)}
      </div>
      <div className="border-t border-gray-200 shrink-0 relative">
        {emojiOpen && <EmojiPicker onSelect={insertEmoji} onClose={() => setEmojiOpen(false)} />}
        <div className="flex items-center gap-2 px-4 pt-2">
          <button disabled={sending} title="表情" onClick={() => setEmojiOpen(!emojiOpen)} className={`p-1.5 rounded ${emojiOpen ? 'text-primary-500 bg-gray-100' : 'text-gray-400 hover:text-gray-600'}`}><FiSmile size={18} /></button>
          <button disabled title="截图：后续版本迁移" className="p-1.5 text-gray-300 cursor-not-allowed"><FiCamera size={18} /></button>
          <button disabled title="图片发送：后续版本迁移" className="p-1.5 text-gray-300 cursor-not-allowed"><FiImage size={18} /></button>
          <button disabled title="文件发送：后续版本迁移" className="p-1.5 text-gray-300 cursor-not-allowed"><FiFile size={18} /></button>
          <span className="text-[11px] text-gray-400 ml-1">核心版暂不支持图片、截图及文件</span>
        </div>
        <div className="px-4 pb-3 pt-1">
          <div ref={editorRef} contentEditable={!sending} suppressContentEditableWarning onKeyDown={handleKeyDown} onKeyUp={saveSelection} onMouseUp={saveSelection} onBlur={saveSelection} onInput={syncHasInput} onPaste={pasteText}
            data-placeholder="输入消息，Enter 发送，Shift+Enter 或 Ctrl+Enter 换行"
            className="chat-editor w-full min-h-[4.5rem] max-h-40 overflow-y-auto text-sm leading-relaxed p-2 rounded border border-gray-200 focus:outline-none focus:ring-1 focus:ring-primary-400 whitespace-pre-wrap break-words" />
          <div className="flex justify-end mt-1"><button onClick={send} disabled={!hasInput || sending} className="px-4 py-1.5 text-sm text-white bg-primary-500 rounded hover:bg-primary-600 disabled:opacity-50 disabled:cursor-not-allowed">{sending ? '发送中…' : '发送'}</button></div>
        </div>
      </div>
      {clearOpen && <ConfirmDialog title="清空聊天记录" message="仅清空本会话在 Rust 版中的记录，不影响原版。此操作无法撤销。" danger onConfirm={() => void clear()} onCancel={() => { if (!clearing.current) setClearOpen(false); }} />}
    </div>
  );
}
