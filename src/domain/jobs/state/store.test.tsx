import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as jobsApi from '../api';
import type { JobInfo } from '../types';
import { useJobs } from './store';

vi.mock('../api');
vi.mock('@/shell', () => ({ showToast: vi.fn() }));

const runningJob: JobInfo = {
  name: 'daily',
  type: 'command',
  enabled: true,
  cron: '* * * * *',
  cronHuman: '每分钟',
  command: 'echo ok',
  prompt: null,
  model: null,
  cwd: null,
  group: null,
  onMissed: 'skip',
  status: 'running',
  lastRun: null,
  nextRun: null,
  running: { startedAt: '2026-07-26T00:00:00Z', pid: 1 },
  verification: 'verified',
  verifiedAt: null,
  specHash: null,
};

describe('useJobs', () => {
  let container: HTMLDivElement;
  let root: Root;
  let current: ReturnType<typeof useJobs>;

  function Harness() {
    current = useJobs();
    return null;
  }

  beforeEach(() => {
    vi.useFakeTimers();
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
    vi.resetAllMocks();
  });

  it('retains the last successful jobs when polling fails', async () => {
    vi.mocked(jobsApi.fetchJobs)
      .mockResolvedValueOnce({ jobs: [runningJob] })
      .mockRejectedValueOnce(new Error('poll failed'));

    await act(async () => root.render(<Harness />));
    await act(async () => {});
    expect(current.jobs).toEqual([runningJob]);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(current.jobs).toEqual([runningJob]);
    expect(current.error).toBe('poll failed');
  });

  it('does not refresh as though a failed action succeeded', async () => {
    vi.mocked(jobsApi.fetchJobs).mockResolvedValue({ jobs: [runningJob] });
    vi.mocked(jobsApi.runJob).mockRejectedValue(new Error('run failed'));

    await act(async () => root.render(<Harness />));
    await act(async () => {});
    await act(async () => { await current.runJob('daily'); });

    expect(current.error).toBe('run failed');
    expect(current.pendingAction).toBeNull();
    expect(jobsApi.fetchJobs).toHaveBeenCalledTimes(1);
  });
});
