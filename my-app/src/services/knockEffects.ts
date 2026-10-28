import { listen } from './bridge';
import { useConfigStore } from '../stores/configStore';
import { useMessageStore } from '../stores/messageStore';
import type { MessageReceivedEvent } from '../types';

// Only live receive events reach this gate. History reads never trigger effects.
export class KnockEffectGate {
  private seen = new Map<string, number>();
  private peers = new Map<string, number>();
  private last = -Infinity;
  accept(data: MessageReceivedEvent, now: number, enabled: boolean): boolean {
    if (data?.type !== 'knock' || !data.id || !data.from || data.from === 'self' ||
        !Number.isFinite(data.timestamp) || Math.abs(now - data.timestamp * 1000) > 5000) return false;
    for (const [id, at] of this.seen) if (now - at > 300000) this.seen.delete(id);
    const key = `${data.from}\0${data.id}`;
    if (this.seen.has(key)) return false;
    if (this.seen.size >= 4096) this.seen.delete(this.seen.keys().next().value!);
    this.seen.set(key, now);
    for (const [peer, at] of this.peers) if (now - at >= 10000) this.peers.delete(peer);
    if (!enabled || this.peers.has(data.from) || now - this.last < 1000) return false;
    this.peers.set(data.from, now); this.last = now;
    return true;
  }
}
const gate = new KnockEffectGate();

export function watchKnockEffects(userId: string, panel: HTMLElement) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const clear = () => { if (timer !== undefined) clearTimeout(timer); timer = undefined; panel.classList.remove('knock-effect'); };
  const stop = listen('message.received', (data: MessageReceivedEvent) => {
    // The message listener runs first; a late event rejected after history was
    // cleared must not produce a visual effect either.
    const stored = data?.id && useMessageStore.getState().messages.get(data.from)?.some((message) => message.id === data.id && message.type === 'knock');
    const enabled = !!stored && useConfigStore.getState().config.knockEffects && data?.from === userId &&
      document.visibilityState === 'visible' && document.hasFocus();
    if (!gate.accept(data, Date.now(), enabled)) return;
    clear(); panel.classList.add('knock-effect'); timer = setTimeout(clear, 500);
  });
  const stopConfig = useConfigStore.subscribe((state) => { if (!state.config.knockEffects) clear(); });
  window.addEventListener('blur', clear);
  document.addEventListener('visibilitychange', clear);
  return () => { stop(); stopConfig(); clear(); window.removeEventListener('blur', clear); document.removeEventListener('visibilitychange', clear); };
}
