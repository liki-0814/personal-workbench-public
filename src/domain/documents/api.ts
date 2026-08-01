import { apiFetch } from '@/core/utils';
import { getBackendUrl } from '@/core/config/backendUrl';
import type {
  DocumentContent,
  DocumentRef,
  EditableDocument,
  MarkdownDocumentContent,
} from './types';
import { validateEditableDocument } from './validation';

interface DocumentResponse<T extends DocumentContent = DocumentContent> {
  success: boolean;
  document: EditableDocument<T>;
}

export async function listDocuments(): Promise<DocumentRef[]> {
  const result = await apiFetch<{ success: boolean; documents: DocumentRef[] }>('/api/documents');
  return result.documents;
}

export async function createDocument<T extends DocumentContent>(title: string, content?: T): Promise<EditableDocument<T>> {
  const result = await apiFetch<DocumentResponse<T>>('/api/documents', {
    method: 'POST',
    body: JSON.stringify({ kind: 'html', title, content }),
  });
  return validateEditableDocument(result.document);
}

export async function createMarkdownDocument(title: string, markdown: string): Promise<EditableDocument<MarkdownDocumentContent>> {
  const result = await apiFetch<DocumentResponse<MarkdownDocumentContent>>('/api/documents', {
    method: 'POST',
    body: JSON.stringify({ kind: 'markdown', title, content: { markdown } }),
  });
  return validateEditableDocument(result.document);
}

export async function readDocument<T extends DocumentContent = DocumentContent>(id: string): Promise<EditableDocument<T>> {
  const result = await apiFetch<DocumentResponse<T>>(`/api/documents/${encodeURIComponent(id)}`);
  return validateEditableDocument(result.document);
}

export async function updateDocument<T extends DocumentContent>(document: EditableDocument<T>): Promise<EditableDocument<T>> {
  const result = await apiFetch<DocumentResponse<T>>(`/api/documents/${encodeURIComponent(document.manifest.id)}`, {
    method: 'PUT',
    body: JSON.stringify({
      expectedRevision: document.manifest.revision,
      title: document.manifest.title,
      content: document.content,
      status: document.manifest.status,
      qaStatus: document.manifest.qaStatus,
    }),
  });
  return validateEditableDocument(result.document);
}

export async function uploadDocumentAsset(id: string, file: File): Promise<{ path: string; url: string }> {
  const form = new FormData();
  form.append('file', file);
  const result = await apiFetch<{ success: boolean; path: string; url: string }>(`/api/documents/${encodeURIComponent(id)}/assets`, {
    method: 'POST',
    body: form,
  });
  return { path: result.path, url: result.url };
}

export async function fetchDocumentAsset(id: string, url: string): Promise<{ path: string; url: string }> {
  const result = await apiFetch<{ success: boolean; path: string; url: string }>(`/api/documents/${encodeURIComponent(id)}/assets/url`, {
    method: 'POST',
    body: JSON.stringify({ url }),
  });
  return { path: result.path, url: result.url };
}

export async function importHtmlDocument(file: File): Promise<EditableDocument> {
  const form = new FormData();
  form.append('file', file);
  const result = await apiFetch<DocumentResponse>('/api/documents/import', { method: 'POST', body: form });
  return validateEditableDocument(result.document);
}

export function documentAssetUrl(documentId: string, relativePath: string): string {
  const asset = relativePath.replace(/^assets\//, '');
  const backend = getBackendUrl() || (typeof globalThis.location === 'object' ? globalThis.location.origin : '');
  return `${backend}/api/document-assets/${encodeURIComponent(documentId)}/${encodeURIComponent(asset)}`;
}

export function documentExportUrl(documentId: string): string {
  return `${getBackendUrl()}/api/documents/${encodeURIComponent(documentId)}/export.html`;
}
