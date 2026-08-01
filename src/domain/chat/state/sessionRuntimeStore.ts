import { useCallback, useEffect, useRef, useState } from 'react';
import { getBackendUrl } from '@/core/config/backendUrl';
import { apiFetch } from '@/core/utils';
import { generateId } from '@/core/utils/id';
import type { ChatAttachment, QueuedMessage, QueuedMessageDelivery, SessionRuntime } from '../types';

export function isSessionTurnRunning(runtime: SessionRuntime | null, localStreaming: boolean): boolean {
  return localStreaming
    || runtime?.phase === 'running'
    || runtime?.phase === 'stopping'
    || Boolean(runtime?.activeTurnId);
}

export function useSessionRuntime(agentSessionId?: string) {
  const [runtime, setRuntime] = useState<SessionRuntime | null>(null);
  const activeSessionRef = useRef(agentSessionId);
  activeSessionRef.current = agentSessionId;

  const applySnapshot = useCallback((snapshot: SessionRuntime) => {
    if (snapshot.sessionId !== activeSessionRef.current) return;
    setRuntime(current => {
      if (
        current?.sessionId === snapshot.sessionId
        && current.lastEventSequence > snapshot.lastEventSequence
      ) {
        return current;
      }
      return snapshot;
    });
  }, []);

  const refresh = useCallback(async () => {
    if (!agentSessionId) {
      if (!activeSessionRef.current) {
        setRuntime(null);
      }
      return;
    }
    const snapshot = await apiFetch<SessionRuntime>(`/api/agent/sessions/${encodeURIComponent(agentSessionId)}/runtime`);
    applySnapshot(snapshot);
  }, [agentSessionId, applySnapshot]);

  useEffect(() => {
    if (!agentSessionId) {
      setRuntime(null);
      return;
    }
    let alive = true;
    setRuntime(null);
    void refresh().catch(() => undefined);
    const source = new EventSource(
      `${getBackendUrl()}/api/agent/sessions/${encodeURIComponent(agentSessionId)}/events?after=0`,
    );
    source.addEventListener('session_runtime', event => {
      if (!alive) return;
      try {
        const snapshot = JSON.parse((event as MessageEvent).data) as SessionRuntime;
        applySnapshot(snapshot);
      } catch {
        // A reconnect fetch will restore the authoritative snapshot.
      }
    });
    source.onerror = () => {
      if (alive) void refresh().catch(() => undefined);
    };
    return () => {
      alive = false;
      source.close();
    };
  }, [agentSessionId, applySnapshot, refresh]);

  const enqueue = useCallback(async (
    content: string,
    delivery: QueuedMessageDelivery,
    imageUrls: string[] = [],
    fileReferences: ChatAttachment[] = [],
  ) => {
    if (!agentSessionId) throw new Error('会话尚未连接 daemon');
    const response = await apiFetch<{ item: QueuedMessage; runtime: SessionRuntime }>(
      `/api/agent/sessions/${encodeURIComponent(agentSessionId)}/inputs`,
      {
        method: 'POST',
        body: JSON.stringify({
          clientMessageId: generateId(),
          delivery,
          message: { role: 'user', content },
          imageUrls,
          fileReferences,
        }),
      },
    );
    applySnapshot(response.runtime);
    return response.item;
  }, [agentSessionId, applySnapshot]);

  const updateItem = useCallback(async (itemId: string, patch: { content?: string; position?: number; delivery?: QueuedMessageDelivery }) => {
    if (!agentSessionId) return;
    await apiFetch(`/api/agent/sessions/${encodeURIComponent(agentSessionId)}/queue/${encodeURIComponent(itemId)}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    });
    await refresh();
  }, [agentSessionId, refresh]);

  const deleteItem = useCallback(async (itemId: string) => {
    if (!agentSessionId) return;
    await apiFetch(`/api/agent/sessions/${encodeURIComponent(agentSessionId)}/queue/${encodeURIComponent(itemId)}`, {
      method: 'DELETE',
    });
    await refresh();
  }, [agentSessionId, refresh]);

  const control = useCallback(async (action: 'abort' | 'resume' | 'send_next' | 'clear_queue') => {
    if (!agentSessionId) return;
    const response = await apiFetch<{ releasedMessage?: { role: 'user'; content: string }; releasedInput?: QueuedMessage; runtime?: SessionRuntime }>(`/api/agent/sessions/${encodeURIComponent(agentSessionId)}/control`, {
      method: 'POST',
      body: JSON.stringify({ action }),
    });
    if (response.runtime) applySnapshot(response.runtime);
    else await refresh();
    return response;
  }, [agentSessionId, applySnapshot, refresh]);

  return { runtime, refresh, enqueue, updateItem, deleteItem, control };
}
