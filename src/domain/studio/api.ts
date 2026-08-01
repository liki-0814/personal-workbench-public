import { apiFetch } from '@/core/utils/apiFetch';
import type { FsEntry, ReviewDiffResponse } from './types';

interface FsListResponse {
  success: boolean;
  entries: FsEntry[];
}

interface FsReadResponse {
  success: boolean;
  content?: string;
  isBinary?: boolean;
  error?: string;
}

interface FsMutationResponse {
  success: boolean;
  error?: string;
}

export interface RuntimeTaskReviewFile {
  taskId: string;
  reviewRevision: number;
  path: string;
  content: string;
  size: number;
  isBinary: false;
}

function assertSuccess(response: FsMutationResponse, fallback: string): void {
  if (!response.success) throw new Error(response.error || fallback);
}

export async function listDirectory(path: string, signal?: AbortSignal): Promise<FsEntry[]> {
  const response = await apiFetch<FsListResponse>(`/api/fs/list?path=${encodeURIComponent(path)}`, { signal });
  assertSuccess(response, '无法读取目录');
  return response.entries ?? [];
}

export async function readTextFile(path: string, signal?: AbortSignal): Promise<{ content: string; isBinary: boolean }> {
  const response = await apiFetch<FsReadResponse>(`/api/fs/read?path=${encodeURIComponent(path)}`, { signal });
  assertSuccess(response, '无法读取文件');
  return {
    content: response.content ?? '',
    isBinary: response.isBinary === true,
  };
}

export async function readRuntimeTaskReviewFile(
  taskId: string,
  path: string,
  signal?: AbortSignal,
): Promise<RuntimeTaskReviewFile> {
  return apiFetch<RuntimeTaskReviewFile>(
    `/api/agent/runtime/tasks/${encodeURIComponent(taskId)}/review-file?path=${encodeURIComponent(path)}`,
    { signal },
  );
}

export async function readRuntimeTaskReviewDiff(
  taskId: string,
  path: string,
  reviewRevision?: number,
  signal?: AbortSignal,
): Promise<ReviewDiffResponse> {
  const params = new URLSearchParams({ path });
  if (reviewRevision != null) params.set('reviewRevision', String(reviewRevision));
  return apiFetch<ReviewDiffResponse>(
    `/api/agent/runtime/tasks/${encodeURIComponent(taskId)}/review-diff?${params.toString()}`,
    { signal },
  );
}

export async function writeTextFile(path: string, content: string): Promise<void> {
  const response = await apiFetch<FsMutationResponse>('/api/fs/write', {
    method: 'POST',
    body: JSON.stringify({ path, content }),
  });
  assertSuccess(response, '保存失败');
}

export async function createDirectory(path: string): Promise<void> {
  const response = await apiFetch<FsMutationResponse>('/api/fs/mkdir', {
    method: 'POST',
    body: JSON.stringify({ path }),
  });
  assertSuccess(response, '新建文件夹失败');
}

export async function deletePath(path: string): Promise<void> {
  const response = await apiFetch<FsMutationResponse>('/api/fs/delete', {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ path }),
  });
  assertSuccess(response, '删除失败');
}
