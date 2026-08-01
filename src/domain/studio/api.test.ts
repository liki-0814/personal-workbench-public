import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  createDirectory,
  listDirectory,
  readRuntimeTaskReviewDiff,
  readRuntimeTaskReviewFile,
} from './api';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('studio fs api', () => {
  it('returns a real empty directory as an empty list', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(
      JSON.stringify({ success: true, entries: [] }),
      { status: 200 },
    )));

    await expect(listDirectory('/workspace')).resolves.toEqual([]);
  });

  it('rejects a failed directory request', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(
      JSON.stringify({ success: false, error: 'Access denied' }),
      { status: 403 },
    )));

    await expect(listDirectory('/outside')).rejects.toThrow('Access denied');
  });

  it('rejects a success-status mutation with an invalid response body', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(
      JSON.stringify({ success: false, error: 'mkdir failed' }),
      { status: 200 },
    )));

    await expect(createDirectory('/workspace/new')).rejects.toThrow('mkdir failed');
  });

  it('reads a worker file through the task-bound endpoint', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      taskId: 'task/1',
      reviewRevision: 2,
      path: 'src/a file.ts',
      content: 'worker latest',
      size: 13,
      isBinary: false,
    }), { status: 200 }));
    vi.stubGlobal('fetch', fetch);

    await expect(readRuntimeTaskReviewFile('task/1', 'src/a file.ts')).resolves.toMatchObject({
      content: 'worker latest',
      reviewRevision: 2,
    });
    expect(String(fetch.mock.calls[0]?.[0])).toContain(
      '/api/agent/runtime/tasks/task%2F1/review-file?path=src%2Fa%20file.ts',
    );
  });

  it('loads a frozen per-file RuntimeTask diff by task and revision', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      taskId: 'task/1',
      reviewRevision: 2,
      patchSha256: 'sha',
      baselineCommit: 'base',
      historyAnomaly: false,
      status: 'pending',
      files: [{
        path: 'src/a file.ts',
        status: 'modified',
        size: 12,
        binary: false,
        truncated: false,
        patch: '@@ -1 +1 @@',
      }],
      previewTruncated: false,
    }), { status: 200 }));
    vi.stubGlobal('fetch', fetch);

    await expect(readRuntimeTaskReviewDiff('task/1', 'src/a file.ts', 2)).resolves.toMatchObject({
      taskId: 'task/1',
      reviewRevision: 2,
    });
    expect(String(fetch.mock.calls[0]?.[0])).toContain(
      '/api/agent/runtime/tasks/task%2F1/review-diff?path=src%2Fa+file.ts&reviewRevision=2',
    );
  });
});
