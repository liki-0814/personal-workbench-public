import { afterEach, describe, expect, it, vi } from 'vitest';
import { apiFetch } from './apiFetch';

vi.mock('@/core/config/backendUrl', () => ({ getBackendUrl: () => '' }));

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('apiFetch', () => {
  it('sets JSON content type for PATCH requests with a JSON body', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('{}', { status: 200 }));
    vi.stubGlobal('fetch', fetchMock);

    await apiFetch('/api/resource', {
      method: 'PATCH',
      body: JSON.stringify({ status: 'viewed' }),
    });

    const options = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(new Headers(options.headers).get('Content-Type')).toBe('application/json');
  });

  it('returns a successful text response without trying to parse it as JSON', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('deleted', {
      status: 200,
      headers: { 'Content-Type': 'text/plain' },
    })));

    await expect(apiFetch<string>('/api/resource', { method: 'DELETE' })).resolves.toBe('deleted');
  });

  it('returns undefined for a successful empty response', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 204 })));

    await expect(apiFetch<void>('/api/resource', { method: 'DELETE' })).resolves.toBeUndefined();
  });
});
