import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { getBackendUrl } from '@/core/config/backendUrl';
import { apiFetch } from '@/core/utils';

export type RuntimeTaskRole = 'researcher' | 'engineer' | 'reviewer' | 'analyst' | 'operator' | 'general';
export type RuntimeTaskAccess = 'read_only' | 'mutating';
export type RuntimeTaskExecutor = 'auto' | 'pwcli' | 'codex' | 'qoder' | 'kimi';
export type RuntimeTaskOutputStatus = 'pending' | 'materializing' | 'ready' | 'failed';
export type RuntimeTaskReviewStatus =
  | 'not_required'
  | 'pending'
  | 'changes_requested'
  | 'applied'
  | 'rejected'
  | 'merge_required';
export type RuntimeTaskReviewAction = 'request_changes' | 'reject' | 'apply' | 'confirm';

export type RuntimeAttentionStatus = 'unread' | 'viewed' | 'resolved';

export interface RuntimeAttention {
  id: string;
  taskId: string;
  sessionId: string;
  workItemId?: string;
  kind: 'document_ready' | 'task_failed' | 'output_failed' | 'merge_required' | 'paused_callback' | string;
  status: RuntimeAttentionStatus;
  revision: number;
  dedupeKey: string;
  title: string;
  detail?: Record<string, unknown>;
  createdAt: string;
  updatedAt: string;
}

export interface RuntimeTaskChangedFile {
  path: string;
  patch?: string;
  status?: string;
}

export interface RuntimeTaskResult {
  summary?: string;
  question?: string;
  options?: string[];
  document?: {
    title?: string;
    format?: 'markdown';
    body?: string;
    id?: string;
  };
  primaryDocumentId?: string;
  changedFiles?: RuntimeTaskChangedFile[];
  tokenUsage?: {
    promptTokens?: number;
    completionTokens?: number;
    totalTokens?: number;
  };
  nativeSessionId?: string;
  suggestedTaskUpdate?: {
    todoId?: string;
    progress?: number;
    status?: string;
    completedSubtaskIds?: string[];
    markComplete?: boolean;
  };
  decisionCandidates?: Array<{
    id?: string;
    title?: string;
    summary?: string;
    content: string;
  }>;
  [key: string]: unknown;
}

export interface RuntimeTask {
  id: string;
  batchId: string;
  rootSessionId: string;
  parentTaskId?: string;
  kind: string;
  objective: string;
  cwd: string;
  backend?: string;
  model?: string;
  status: string;
  deliveryStatus: string;
  attempt: number;
  createdAt: string;
  updatedAt: string;
  role?: RuntimeTaskRole;
  access?: RuntimeTaskAccess;
  executorRequest?: RuntimeTaskExecutor;
  resolvedExecutorKind?: 'internal_agent' | 'acp_cli';
  resolvedExecutorId?: Exclude<RuntimeTaskExecutor, 'auto'>;
  resolvedModel?: string;
  resolvedEffort?: string;
  resolvedPermissionMode?: string;
  routingReason?: string;
  displayName?: string;
  roleLabel?: string;
  avatarSeed?: string;
  deliverableTitle?: string;
  primaryDocumentId?: string;
  outputStatus?: RuntimeTaskOutputStatus;
  reviewStatus?: RuntimeTaskReviewStatus;
  reviewRevision?: number;
  depth?: number;
  workItemId?: string;
  sessionGeneration?: number;
  result?: RuntimeTaskResult | unknown;
  error?: string;
  failure?: import('../types').FailureEnvelope;
  metadata?: unknown;
}

export interface RuntimeTaskEvent {
  sequence: number;
  eventId: string;
  taskId: string;
  attemptId?: string;
  kind: string;
  payload?: unknown;
  createdAt: string;
}

export function selectRuntimeTasks(tasks: RuntimeTask[], rootSessionId?: string): RuntimeTask[] {
  return tasks.filter(task => (
    (!rootSessionId || task.rootSessionId === rootSessionId)
    && task.kind !== 'supervisor_dispatch'
  ));
}

export function useTaskRuntime(rootSessionId?: string) {
  const [tasks, setTasks] = useState<RuntimeTask[]>([]);
  const [attention, setAttention] = useState<RuntimeAttention[]>([]);
  const [eventsByTaskId, setEventsByTaskId] = useState<Record<string, RuntimeTaskEvent[]>>({});
  const [loaded, setLoaded] = useState(false);
  const cursorRef = useRef(0);

  const refresh = useCallback(async () => {
    try {
      const [snapshot, attentionSnapshot] = await Promise.all([
        apiFetch<RuntimeTask[]>('/api/agent/runtime/snapshot'),
        apiFetch<RuntimeAttention[]>('/api/agent/attention?includeResolved=true'),
      ]);
      setTasks(snapshot);
      setAttention(attentionSnapshot);
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    let alive = true;
    let source: EventSource | null = null;
    let refreshTimer: number | null = null;
    let reconnectTimer: number | null = null;
    void refresh().catch(() => undefined);
    const scheduleRefresh = () => {
      if (refreshTimer != null) return;
      refreshTimer = window.setTimeout(() => {
        refreshTimer = null;
        if (alive) void refresh().catch(() => undefined);
      }, 100);
    };
    const connect = () => {
      if (!alive) return;
      source = new EventSource(`${getBackendUrl()}/api/agent/runtime/events?after=${cursorRef.current}`);
      const onEvent = (event: Event) => {
        const sequence = Number((event as MessageEvent).lastEventId);
        if (Number.isFinite(sequence)) cursorRef.current = Math.max(cursorRef.current, sequence);
        try {
          const taskEvent = JSON.parse((event as MessageEvent).data) as RuntimeTaskEvent;
          if (taskEvent.taskId) {
            setEventsByTaskId(current => {
              const existing = current[taskEvent.taskId] ?? [];
              if (existing.some(item => item.eventId === taskEvent.eventId)) return current;
              return {
                ...current,
                [taskEvent.taskId]: [...existing, taskEvent].slice(-20),
              };
            });
          }
        } catch {
          // Snapshot refresh still keeps task state correct when an older daemon emits a non-JSON event.
        }
        scheduleRefresh();
      };
      [
        'task_published', 'task_leased', 'task_started', 'task_completed',
        'task_failed', 'task_cancelled', 'task_waiting_children',
        'task_waiting_configuration', 'task_waiting_user', 'task_decision_resolved',
        'task_native_session_started', 'task_follow_up_queued',
        'task_child_result_buffered', 'task_child_results_queued', 'task_children_ready',
        'task_recovery_required', 'task_start_failed',
        'task_output_materializing',
        'task_output_ready', 'task_output_failed', 'task_review_updated',
        'attention_created', 'attention_updated', 'attention_deleted', 'legacy_dispatch_migrated',
      ].forEach(kind => source?.addEventListener(kind, onEvent));
      source.onerror = () => {
        source?.close();
        source = null;
        if (reconnectTimer == null && alive) {
          reconnectTimer = window.setTimeout(() => {
            reconnectTimer = null;
            connect();
          }, 1000);
        }
      };
    };
    connect();
    return () => {
      alive = false;
      source?.close();
      if (refreshTimer != null) window.clearTimeout(refreshTimer);
      if (reconnectTimer != null) window.clearTimeout(reconnectTimer);
    };
  }, [refresh]);

  const sessionTasks = useMemo(
    () => selectRuntimeTasks(tasks, rootSessionId),
    [rootSessionId, tasks],
  );

  const cancel = useCallback(async (taskId: string) => {
    await apiFetch(`/api/agent/runtime/tasks/${encodeURIComponent(taskId)}/cancel`, { method: 'POST' });
    await refresh();
  }, [refresh]);

  const retry = useCallback(async (taskId: string, executor?: Exclude<RuntimeTaskExecutor, 'auto'>) => {
    await apiFetch(`/api/agent/runtime/tasks/${encodeURIComponent(taskId)}/retry`, {
      method: 'POST',
      ...(executor ? { body: JSON.stringify({ executor }) } : {}),
    });
    await refresh();
  }, [refresh]);

  const resolveDecision = useCallback(async (taskId: string, option: string, note?: string) => {
    await apiFetch(`/api/agent/runtime/tasks/${encodeURIComponent(taskId)}/decision`, {
      method: 'POST',
      body: JSON.stringify({ clientActionId: crypto.randomUUID(), option, note }),
    });
    await refresh();
  }, [refresh]);

  const followUp = useCallback(async (taskId: string, objective: string) => {
    await apiFetch(`/api/agent/runtime/tasks/${encodeURIComponent(taskId)}/follow-up`, {
      method: 'POST',
      body: JSON.stringify({ objective }),
    });
    await refresh();
  }, [refresh]);

  const review = useCallback(async (
    taskId: string,
    action: RuntimeTaskReviewAction,
    reviewRevision: number,
    note?: string,
    projection?: {
      taskUpdate?: RuntimeTaskResult['suggestedTaskUpdate'];
      memoryEntries?: RuntimeTaskResult['decisionCandidates'];
    },
  ) => {
    await apiFetch(`/api/agent/runtime/tasks/${encodeURIComponent(taskId)}/review`, {
      method: 'POST',
      body: JSON.stringify({
        clientActionId: crypto.randomUUID(),
        reviewRevision,
        action,
        note,
        taskUpdate: projection?.taskUpdate,
        memoryEntries: projection?.memoryEntries,
      }),
    });
    await refresh();
  }, [refresh]);

  const updateAttention = useCallback(async (
    attentionId: string,
    status: RuntimeAttentionStatus,
  ) => {
    const current = attention.find(item => item.id === attentionId);
    const updated = await apiFetch<RuntimeAttention>(`/api/agent/attention/${encodeURIComponent(attentionId)}`, {
      method: 'PATCH',
      body: JSON.stringify({ status, expectedRevision: current?.revision }),
    });
    setAttention(items => items.some(item => item.id === attentionId)
      ? items.map(item => item.id === attentionId ? updated : item)
      : [updated, ...items]);
  }, [attention]);

  const markAttentionViewed = useCallback(
    async (attentionId: string) => updateAttention(attentionId, 'viewed'),
    [updateAttention],
  );

  const resolveAttention = useCallback(
    async (attentionId: string) => updateAttention(attentionId, 'resolved'),
    [updateAttention],
  );

  const deleteAttention = useCallback(async (attentionId: string) => {
    const updated = await apiFetch<RuntimeAttention>(`/api/agent/attention/${encodeURIComponent(attentionId)}`, {
      method: 'DELETE',
    });
    setAttention(items => items.some(item => item.id === attentionId)
      ? items.map(item => item.id === attentionId ? updated : item)
      : [updated, ...items]);
  }, []);

  return {
    tasks: sessionTasks,
    eventsByTaskId,
    attention,
    loaded,
    cancel,
    retry,
    resolveDecision,
    followUp,
    review,
    markAttentionViewed,
    resolveAttention,
    deleteAttention,
    refresh,
  };
}

export type TaskRuntimeSnapshot = ReturnType<typeof useTaskRuntime>;
