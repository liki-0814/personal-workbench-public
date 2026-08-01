import { afterEach, describe, expect, it, vi } from 'vitest';
import { captureWorkItemSession, type WorkItemCaptureRequest } from './workItemCapture';

vi.mock('@/core/config/backendUrl', () => ({ getBackendUrl: () => '' }));

describe('captureWorkItem', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('posts the complete request and loads the exact returned session', async () => {
    const request: WorkItemCaptureRequest = {
      clientRequestId: 'capture-1',
      prompt: '完成支付链路回归并整理结果',
      cwd: '/workspace/project',
      executorPreference: 'codex',
    };
    const response = {
      clientRequestId: 'capture-1',
      workItemId: 'todo-1',
      sessionId: 'session-1',
      queuedInputId: 'queue-1',
      created: true,
    };
    const snapshot = {
      id: 'session-1',
      name: 'Capture: 完成支付链路回归并整理结果',
      messages: [],
      cwd: '/workspace/project',
    };
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(response), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(snapshot), { status: 200 }));
    vi.stubGlobal('fetch', fetchMock);

    await expect(captureWorkItemSession(request)).resolves.toEqual(snapshot);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    const [path, options] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(path).toBe('/api/agent/work-items/capture');
    expect(options.method).toBe('POST');
    expect(JSON.parse(String(options.body))).toEqual(request);
    expect(fetchMock.mock.calls[1][0]).toBe('/api/agent/sessions/session-1/snapshot');
  });
});
