import { apiFetch } from '@/core/utils';
import type { ChatMessage } from '@/domain/chat';

export type WorkItemExecutor = 'auto' | 'pwcli' | 'codex' | 'qoder' | 'kimi';

export interface WorkItemCaptureRequest {
  clientRequestId: string;
  prompt: string;
  cwd: string;
  executorPreference: WorkItemExecutor;
}

export interface WorkItemCaptureResponse {
  clientRequestId: string;
  workItemId: string;
  sessionId: string;
  queuedInputId: string;
  runtimeTaskId?: string;
  created: boolean;
}

export interface CapturedSessionSnapshot {
  id: string;
  name: string;
  messages: ChatMessage[];
  cwd?: string;
}

export async function captureWorkItemSession(request: WorkItemCaptureRequest): Promise<CapturedSessionSnapshot> {
  const captured = await apiFetch<WorkItemCaptureResponse>('/api/agent/work-items/capture', {
    method: 'POST',
    body: JSON.stringify(request),
  });
  return apiFetch<CapturedSessionSnapshot>(
    `/api/agent/sessions/${encodeURIComponent(captured.sessionId)}/snapshot`,
  );
}
