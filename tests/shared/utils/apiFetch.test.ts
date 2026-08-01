import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { apiFetch } from '@/core/utils/apiFetch';

describe('apiFetch', () => {
  beforeEach(() => vi.stubGlobal('__PWB_BACKEND_URL__', ''));
  afterEach(() => vi.unstubAllGlobals());

  it('accepts successful responses with an empty body', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 202 })));

    await expect(apiFetch('/accepted', { method: 'POST' })).resolves.toBeUndefined();
  });

  it('still parses JSON responses', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json({ ok: true })));

    await expect(apiFetch<{ ok: boolean }>('/json')).resolves.toEqual({ ok: true });
  });
});
