import { useEffect, useRef } from 'react';
import { STORAGE_SYNC_EVENT } from './syncEngine';

/** Subscribe to storage sync events.
 *  When backend data is pulled into localStorage (via syncFromServer),
 *  this fires the reload callback so the store can re-read its persisted
 *  state without re-mounting the component tree.
 */
export function useStorageSync(reload: () => void): void {
  const reloadRef = useRef(reload);
  reloadRef.current = reload;
  useEffect(() => {
    const handler = () => reloadRef.current();
    window.addEventListener(STORAGE_SYNC_EVENT, handler);
    return () => window.removeEventListener(STORAGE_SYNC_EVENT, handler);
  }, []);
}
