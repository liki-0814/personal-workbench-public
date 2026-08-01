import { useState, useEffect, useCallback } from 'react';
import { apiFetch } from '@/core/utils';

export interface BackgroundTaskInfo {
  id: string;
  kind?: 'background' | 'supervisor';
  sessionId: string;
  /** 前端 ChatSession.id（pwcli session name），用于回调匹配 */
  sessionName?: string;
  toolName: string;
  description: string;
  status: 'running' | 'completed' | 'failed' | 'cancelled';
  elapsedSecs?: number;
  success?: boolean;
  summary?: string;
  logFile?: string;
  durationSecs?: number;
  replayable?: boolean;
  phase?: string;
  requiresAttention?: boolean;
  updatedAt?: string;
  runtimeStatus?: string;
}

export function useBackgroundTasks() {
  const [tasks, setTasks] = useState<BackgroundTaskInfo[]>([]);

  const mergeTaskList = useCallback((incoming: BackgroundTaskInfo[]) => {
    if (!Array.isArray(incoming) || incoming.length === 0) return;
    setTasks(prev => {
      let changed = false;
      const merged = [...prev];
      for (const task of incoming) {
        const idx = merged.findIndex(t => t.id === task.id);
        if (idx >= 0) {
          if (merged[idx].status !== task.status || merged[idx].elapsedSecs !== task.elapsedSecs) {
            merged[idx] = task;
            changed = true;
          }
        } else {
          merged.push(task);
          changed = true;
        }
      }
      return changed ? merged : prev;
    });
  }, []);

  useEffect(() => {
    let alive = true;

    void apiFetch<BackgroundTaskInfo[]>('/api/agent/background-tasks')
      .then(list => {
        if (!alive || !Array.isArray(list)) return;
        mergeTaskList(list);
      })
      .catch(() => undefined);

    let es: EventSource | null = null;
    try {
      es = new EventSource('/api/agent/background-events');
      es.addEventListener('background_task_started', (ev) => {
        try {
          const payload = JSON.parse((ev as MessageEvent).data);
          const task: BackgroundTaskInfo = {
            id: payload.taskId,
            sessionId: payload.sessionId,
            toolName: payload.toolName,
            description: payload.description,
            status: 'running',
          };
          mergeTaskList([task]);
        } catch { /* ignore */ }
      });
      es.addEventListener('background_task_completed', (ev) => {
        try {
          const payload = JSON.parse((ev as MessageEvent).data);
          const task: BackgroundTaskInfo = {
            id: payload.taskId,
            sessionId: payload.sessionId,
            toolName: payload.toolName,
            description: payload.toolName,
            status: payload.success ? 'completed' : 'failed',
            success: payload.success,
            summary: payload.summary,
            logFile: payload.logFile,
            durationSecs: payload.durationSecs,
          };
          mergeTaskList([task]);
        } catch { /* ignore */ }
      });
      es.addEventListener('background_task_cancelled', (ev) => {
        try {
          const payload = JSON.parse((ev as MessageEvent).data);
          mergeTaskList([{
            id: payload.taskId,
            sessionId: payload.sessionId,
            toolName: payload.toolName,
            description: payload.toolName,
            status: 'cancelled',
            summary: payload.summary,
          }]);
        } catch { /* ignore */ }
      });
    } catch { /* ignore */ }

    return () => {
      alive = false;
      es?.close();
    };
  }, [mergeTaskList]);

  const cancel = useCallback(async (taskId: string) => {
    setTasks(prev => prev.map(t => t.id === taskId ? { ...t, status: 'cancelled' as const } : t));
    await apiFetch(`/api/agent/background-tasks/${taskId}`, { method: 'DELETE' }).catch(() => {});
  }, []);

  const cancelBySession = useCallback(async (sid: string) => {
    setTasks(prev => prev.map(t =>
      t.sessionId === sid && t.status === 'running' ? { ...t, status: 'cancelled' as const } : t
    ));
    await apiFetch(`/api/agent/sessions/${sid}/background-tasks`, { method: 'DELETE' }).catch(() => {});
  }, []);

  const dismiss = useCallback(async (taskId: string) => {
    setTasks(prev => prev.filter(t => t.id !== taskId));
    await apiFetch(`/api/agent/background-tasks/${taskId}`, { method: 'DELETE' }).catch(() => {});
  }, []);

  const allTasks = tasks;
  const activeCount = tasks.filter(task => task.status === 'running').length;
  const runningCount = activeCount;

  return { allTasks, cancel, cancelBySession, dismiss, runningCount, activeCount };
}
