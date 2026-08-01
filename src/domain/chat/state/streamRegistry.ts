import { useCallback, useSyncExternalStore } from 'react';

interface StreamRecord {
  controller: AbortController;
  loading: boolean;
  error: string | null;
}

interface StreamSnapshot {
  loading: boolean;
  error: string | null;
}

const records = new Map<string, StreamRecord>();
const snapshots = new Map<string, StreamSnapshot>();
const listeners = new Set<() => void>();

const EMPTY_SNAPSHOT: StreamSnapshot = { loading: false, error: null };

function refreshSnapshot(sessionId: string): StreamSnapshot {
  const rec = records.get(sessionId);
  const loading = rec?.loading ?? false;
  const error = rec?.error ?? null;
  const cached = snapshots.get(sessionId);
  if (cached && cached.loading === loading && cached.error === error) {
    return cached;
  }
  const fresh: StreamSnapshot = { loading, error };
  snapshots.set(sessionId, fresh);
  return fresh;
}

function notify() {
  for (const listener of listeners) listener();
}

export function startStream(sessionId: string): AbortController {
  const existing = records.get(sessionId);
  existing?.controller.abort();
  const controller = new AbortController();
  records.set(sessionId, { controller, loading: true, error: null });
  refreshSnapshot(sessionId);
  notify();
  return controller;
}

export function endStream(sessionId: string) {
  const rec = records.get(sessionId);
  if (!rec) return;
  records.set(sessionId, { ...rec, loading: false });
  refreshSnapshot(sessionId);
  notify();
}

export function abortStream(sessionId: string) {
  const rec = records.get(sessionId);
  if (!rec) return;
  rec.controller.abort();
  records.set(sessionId, { ...rec, loading: false });
  refreshSnapshot(sessionId);
  notify();
}

export function setStreamError(sessionId: string, error: string | null) {
  const rec = records.get(sessionId);
  if (rec) {
    records.set(sessionId, { ...rec, error });
  } else {
    records.set(sessionId, { controller: new AbortController(), loading: false, error });
  }
  refreshSnapshot(sessionId);
  notify();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useStreamState(sessionId: string | undefined): StreamSnapshot {
  const getSnapshot = useCallback(() => {
    if (!sessionId) return EMPTY_SNAPSHOT;
    return refreshSnapshot(sessionId);
  }, [sessionId]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
