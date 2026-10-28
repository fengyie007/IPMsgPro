import { invoke, listen } from './bridge';
import { useUserStore } from '../stores/userStore';
import type { User } from '../types';

// Serialize reads of the one-shot native activation so startup and click events cannot steal it from each other.
export function watchNotificationActivation(open: (user: User) => void): () => void {
  let disposed = false, working = false, again = false;
  const drain = async () => {
    if (working) { again = true; return; }
    working = true;
    try {
      do {
        again = false;
        const result = await invoke<{ userId: string | null }>('notification.take_activation');
        if (disposed) return;
        if (result.userId) {
          let user = useUserStore.getState().users.find((u) => u.id === result.userId);
          if (!user) { await useUserStore.getState().loadUsers(); user = useUserStore.getState().users.find((u) => u.id === result.userId); }
          if (user && !disposed) open(user);
        }
      } while (again && !disposed);
    } catch (error) { console.error('Notification activation failed', error); }
    finally { working = false; }
  };
  const onFocus = () => void drain();
  const stop = listen('notification.activated', onFocus); window.addEventListener('focus', onFocus); void drain();
  return () => { disposed = true; stop(); window.removeEventListener('focus', onFocus); };
}
