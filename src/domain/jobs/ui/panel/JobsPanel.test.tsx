import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import JobsPanel, { LogViewer } from './JobsPanel';
import JobCard from './JobCard';
import type { JobInfo } from '../../types';

describe('LogViewer', () => {
  let container: HTMLDivElement | null = null;

  afterEach(() => {
    vi.unstubAllGlobals();
    container?.remove();
    container = null;
  });

  it('aborts the pending log request when unmounted', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    let requestSignal: AbortSignal | undefined;
    const getLogs = vi.fn((_name: string, signal?: AbortSignal) => {
      requestSignal = signal;
      return new Promise<string[]>(() => {});
    });

    await act(async () => {
      root.render(<LogViewer
        name="daily"
        onClose={() => {}}
        getLogs={getLogs}
        getLogContent={vi.fn()}
        onDeleteLog={vi.fn()}
      />);
    });
    expect(requestSignal?.aborted).toBe(false);

    await act(async () => root.unmount());
    expect(requestSignal?.aborted).toBe(true);
  });

  it('does not present a failed initial request as an empty jobs list', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(
      JSON.stringify({ success: false, error: 'jobs unavailable' }),
      { status: 500 },
    )));
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);

    await act(async () => root.render(<JobsPanel />));
    await act(async () => {});
    expect(container.textContent).toContain('jobs unavailable');
    expect(container.textContent).not.toContain('暂无调度任务');
    await act(async () => root.unmount());
  });

  it('keeps the delete action out of the collapsed row and reachable once expanded', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    const onDelete = vi.fn();
    const job: JobInfo = {
      name: 'daily-summary',
      type: 'agent',
      enabled: true,
      cron: '0 9 * * *',
      cronHuman: '每天 09:00',
      command: null,
      prompt: '汇总昨日事项',
      model: null,
      cwd: null,
      group: null,
      onMissed: 'skip',
      status: 'idle',
      lastRun: null,
      nextRun: null,
      running: null,
      verification: 'verified',
      verifiedAt: null,
      specHash: null,
    };

    await act(async () => root.render(
      <JobCard
        job={job}
        onRun={() => {}}
        onToggle={() => {}}
        onDelete={onDelete}
        onViewLogs={() => {}}
        pending={false}
      />,
    ));

    expect(container.querySelector<HTMLButtonElement>('button[aria-label="删除daily-summary"]')).toBeNull();

    const headerButton = container.querySelector<HTMLButtonElement>('button[aria-expanded="false"]');
    expect(headerButton).not.toBeNull();
    await act(async () => headerButton?.click());

    const deleteButton = container.querySelector<HTMLButtonElement>('button[aria-label="删除daily-summary"]');
    expect(deleteButton).not.toBeNull();
    await act(async () => deleteButton?.click());
    expect(onDelete).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
  });
});
