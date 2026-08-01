import { afterEach, describe, expect, it, vi } from 'vitest';
import { deleteJob, fetchJobLogs, fetchJobs } from './api';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('jobs api', () => {
  it('returns jobs from a successful response', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(
      JSON.stringify({ jobs: [{ name: 'daily' }] }),
      { status: 200 },
    )));

    await expect(fetchJobs()).resolves.toEqual({ jobs: [{ name: 'daily' }] });
  });

  it('throws the server error instead of treating HTTP 500 as empty data', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(
      JSON.stringify({ success: false, error: 'jobs unavailable' }),
      { status: 500 },
    )));

    await expect(fetchJobs()).rejects.toThrow('jobs unavailable');
  });

  it('encodes job and log names and checks destructive requests', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ logs: ['a.log'] }), { status: 200 }))
      .mockResolvedValueOnce(new Response('', { status: 200 }));
    vi.stubGlobal('fetch', fetchMock);

    await expect(fetchJobLogs('job/name')).resolves.toEqual(['a.log']);
    await deleteJob('job/name');
    expect(fetchMock.mock.calls[0][0]).toContain('job%2Fname/logs');
    expect(fetchMock.mock.calls[1][1]).toMatchObject({ method: 'DELETE' });
  });
});
