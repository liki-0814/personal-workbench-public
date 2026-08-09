import { apiFetch } from '@/core/utils/apiFetch';
import { getBackendUrl } from '@/core/config/backendUrl';
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

export interface JobProposalSpec {
  name: string;
  type: 'command' | 'agent';
  cron: string;
  command?: string;
  prompt?: string;
  cwd?: string;
  model?: string;
  testCommand?: string;
  approvedTest?: boolean;
}

export type ProposeJobResult =
  | { kind: 'applied'; jobs: JobInfo[] }
  | { kind: 'approval_required'; reason: string }
  | { kind: 'test_failed'; summary: string }
  | { kind: 'error'; message: string };

interface ProposalResponse {
  success?: boolean;
  error?: string;
  reason?: string;
  jobs?: JobInfo[];
  proposal?: {
    testSummary?: string;
  };
}

/**
 * Submit a job proposal: the backend validates it, runs a real test
 * (synchronously, up to several minutes), and only then writes the job config.
 * Uses raw fetch because the 409/422 bodies carry proposal details that
 * apiFetch would discard.
 */
export async function proposeJob(spec: JobProposalSpec): Promise<ProposeJobResult> {
  const body: Record<string, unknown> = {
    name: spec.name,
    cron: spec.cron,
    type: spec.type,
  };
  if (spec.command) body.command = spec.command;
  if (spec.prompt) body.prompt = spec.prompt;
  if (spec.cwd) body.cwd = spec.cwd;
  if (spec.model) body.model = spec.model;
  if (spec.testCommand) body.testCommand = spec.testCommand;
  if (spec.approvedTest) body.approvedTest = true;

  let parsed: ProposalResponse = {};
  try {
    const response = await fetch(`${getBackendUrl()}/api/jobs/proposals`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    parsed = (await response.json().catch(() => ({}))) as ProposalResponse;
    if (response.ok && parsed.success) {
      return { kind: 'applied', jobs: parsed.jobs ?? [] };
    }
    if (response.status === 409 && parsed.error === 'job_test_approval_required') {
      return {
        kind: 'approval_required',
        reason: parsed.reason || '测试会真实执行该命令，可能产生副作用',
      };
    }
    if (parsed.error === 'job_test_failed') {
      return {
        kind: 'test_failed',
        summary: parsed.proposal?.testSummary || '测试执行失败，未通过验证',
      };
    }
    return {
      kind: 'error',
      message: typeof parsed.error === 'string' ? parsed.error : `创建失败（${response.status}）`,
    };
  } catch (reason) {
    return {
      kind: 'error',
      message: reason instanceof Error ? reason.message : '创建请求失败',
    };
  }
}
