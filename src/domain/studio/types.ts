import type { DocumentRef } from '@/domain/documents';

export interface FsEntry {
  name: string;
  type: 'file' | 'directory';
  size?: number;
}

export interface EditorTab {
  id: string;
  path: string;
  name: string;
  content: string;
  originalContent: string;
  language: string;
  dirty: boolean;
}

export interface WorkspaceRef {
  path: string;
  line?: number;
  kind: 'file' | 'diff';
  taskId?: string;
  supervisorTaskId?: string;
  childId?: string;
  workspaceRoot?: string;
  reviewRevision?: number;
  /** Worker-captured task-local unified diff; avoids coupling review links to Supervisor. */
  inlinePatch?: string;
}

export interface ReviewDiffResponse {
  taskId: string;
  childId?: string;
  reviewRevision: number;
  patchSha256: string;
  baselineCommit: string;
  historyAnomaly: boolean;
  status: string;
  files: Array<{
    path: string;
    status: string;
    size: number;
    sha256?: string;
    binary: boolean;
    truncated: boolean;
    patch?: string;
  }>;
  previewTruncated: boolean;
}

export type ContextPaneMode = 'collaboration' | 'attention' | 'review' | 'studio' | 'document';

export interface ContextPaneState {
  open: boolean;
  mode: ContextPaneMode;
  runtimeBatchId?: string;
  supervisorTaskId?: string;
  attentionId?: string;
  reviewRevision?: number;
  workspaceRef?: WorkspaceRef;
  documentRef?: DocumentRef;
}
