import { apiFetch } from '@/core/utils/apiFetch';
import type { JobInfo } from './types';

interface JobsResponse {
  jobs: JobInfo[];
}

interface LogsResponse {
  logs: string[];
}

interface LogContentResponse {
  content: string;
}

export function fetchJobs(signal?: AbortSignal): Promise<JobsResponse> {
  return apiFetch('/api/jobs', { signal });
}

export function refreshJobs(signal?: AbortSignal): Promise<JobsResponse> {
  return apiFetch('/api/jobs/refresh', { method: 'POST', signal });
}

export async function runJob(name: string): Promise<void> {
  await apiFetch(`/api/jobs/${encodeURIComponent(name)}/run`, { method: 'POST' });
}

export async function toggleJob(name: string): Promise<void> {
  await apiFetch(`/api/jobs/${encodeURIComponent(name)}/toggle`, { method: 'PUT' });
}

export async function deleteJob(name: string): Promise<void> {
  await apiFetch(`/api/jobs/${encodeURIComponent(name)}`, { method: 'DELETE' });
}

export async function fetchJobLogs(name: string, signal?: AbortSignal): Promise<string[]> {
  const response = await apiFetch<LogsResponse>(`/api/jobs/${encodeURIComponent(name)}/logs`, { signal });
  return response.logs ?? [];
}

export async function fetchJobLogContent(name: string, file: string, signal?: AbortSignal): Promise<string> {
  const response = await apiFetch<LogContentResponse>(
    `/api/jobs/${encodeURIComponent(name)}/logs/${encodeURIComponent(file)}`,
    { signal },
  );
  return response.content ?? '';
}

export async function deleteJobLog(name: string, file: string): Promise<void> {
  await apiFetch(`/api/jobs/${encodeURIComponent(name)}/logs/${encodeURIComponent(file)}`, {
    method: 'DELETE',
  });
}
