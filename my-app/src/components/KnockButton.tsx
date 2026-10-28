import { useEffect, useState } from 'react';
import { FiZap } from 'react-icons/fi';
import { useMessageStore } from '../stores/messageStore';
import { toast } from '../stores/toastStore';

export default function KnockButton({ target, disabled }: { target: string; disabled: boolean }) {
  const until = useMessageStore((state) => state.knockUntil.get(target) || 0);
  const pending = useMessageStore((state) => state.knockPending.has(target));
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    setNow(Date.now());
    if (until <= Date.now()) return;
    const timer = window.setInterval(() => {
      setNow(Date.now());
      if (Date.now() >= until) window.clearInterval(timer);
    }, 250);
    return () => window.clearInterval(timer);
  }, [until]);
  const seconds = Math.max(0, Math.ceil((until - now) / 1000));
  const title = seconds ? `窗口振动（${seconds}秒后可再次发送）` : '发送窗口振动';
  return <button type="button" aria-label={title} title={title} disabled={disabled || pending || seconds > 0}
    className="p-1.5 text-gray-400 hover:text-gray-600 disabled:opacity-40" onClick={async () => {
      if (!await useMessageStore.getState().sendKnock(target)) toast.error(useMessageStore.getState().error || '窗口振动发送失败');
    }}><FiZap size={18} /></button>;
}
