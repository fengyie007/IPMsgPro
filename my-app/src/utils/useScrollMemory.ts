import { useLayoutEffect, useRef } from 'react';
import { useWorkspaceStore } from '../stores/workspaceStore';

export function useScrollMemory(key: string) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (ref.current) ref.current.scrollTop = useWorkspaceStore.getState().scrollPositions.get(key) || 0;
  }, [key]);
  const onScroll = () => {
    if (ref.current?.clientHeight) useWorkspaceStore.getState().saveScroll(key, ref.current.scrollTop);
  };
  return { ref, onScroll };
}
