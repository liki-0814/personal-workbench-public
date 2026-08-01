export type DocumentKind = 'html' | 'markdown';
export type DocumentStatus = 'draft' | 'ready';
export type DocumentQaStatus = 'pending' | 'passed' | 'warning';
export type DocumentRuntime = 'static' | 'archify' | 'delegated';

export interface RuntimeTaskDocumentOrigin {
  type: 'runtime_task';
  taskId: string;
  attemptId: string;
  batchId: string;
  executorId: string;
  agentName: string;
}

export type DocumentOrigin = RuntimeTaskDocumentOrigin;

export interface DocumentRef {
  id: string;
  kind: DocumentKind;
  title: string;
  revision: number;
  status: string;
  qaStatus: string;
  runtime?: DocumentRuntime;
  origin?: DocumentOrigin;
}

export interface DocumentManifest extends DocumentRef {
  schemaVersion: number;
  updatedAt: string;
}

export interface HtmlDocumentContent {
  html: string;
}

export interface MarkdownDocumentContent {
  markdown: string;
}

export type DocumentContent = HtmlDocumentContent | MarkdownDocumentContent;

export interface TaskDeliverable {
  documentId: string;
  relation: 'primary' | 'supporting';
  state: 'materializing' | 'ready' | 'failed';
}

export interface EditableDocument<T extends DocumentContent = DocumentContent> {
  manifest: DocumentManifest;
  content: T;
}
