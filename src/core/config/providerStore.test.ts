import { beforeEach, describe, expect, it, vi } from 'vitest';

const apiFetch = vi.hoisted(() => vi.fn());
vi.mock('@/core/utils', () => ({ apiFetch }));

import {
  completeProviderAuth,
  getProviderSnapshot,
  refreshProviders,
  reorderProviderIds,
  startProviderAuth,
  updateProviderModels,
} from './providerStore';
import { getModels } from './aiProviders';

describe('providerStore', () => {
  beforeEach(() => {
    localStorage.clear();
    apiFetch.mockReset();
  });

  it('unwraps server views and updates the model compatibility view without storing credentials', async () => {
    localStorage.setItem('pwb_ai_providers', JSON.stringify([{ name: 'legacy', apiKey: 'secret' }]));
    apiFetch.mockImplementation(async (path: string) => {
      if (path === '/api/providers') return { success: true, data: [{
        id: 'custom-1', kind: 'custom', name: 'Gateway',
        auth: { method: 'api_key', status: 'connected' },
        models: [{ id: 'model-a', name: 'Model A' }], defaultModel: 'model-a',
        enabled: true, priority: 0, builtin: false,
        customEndpoint: { baseUrl: 'https://example.test/v1', protocol: 'openai_responses', useProxy: true },
      }] };
      if (path === '/api/providers/catalog') return { success: true, data: [{
        kind: 'kimi-coding', name: 'Kimi Coding', authMethod: 'oauth', defaultModel: 'kimi-for-coding', models: [],
      }, {
        kind: 'google-antigravity', name: 'Google Antigravity', authMethod: 'oauth', models: [],
        available: false, unavailableReason: 'OAuth client missing',
      }] };
      throw new Error(`unexpected ${path}`);
    });

    await refreshProviders();
    const provider = getProviderSnapshot().providers[0];
    expect(provider).toMatchObject({
      id: 'custom-1', baseUrl: 'https://example.test/v1', protocol: 'openai_responses', useProxy: true,
    });
    expect(getModels().map(model => model.id)).toContain('model-a');
    expect(getProviderSnapshot().catalog.find(entry => entry.kind === 'google-antigravity')).toMatchObject({
      available: false,
      unavailableReason: 'OAuth client missing',
    });
    expect(localStorage.getItem('pwb_ai_providers')).toBeNull();
  });

  it('keeps configured providers usable when model discovery is unavailable', async () => {
    apiFetch.mockImplementation(async (path: string) => {
      if (path === '/api/providers') return { success: true, data: [{
        id: 'codex-1', kind: 'openai-codex', name: 'OpenAI Codex',
        auth: { method: 'oauth', status: 'disconnected' },
        models: [], defaultModel: 'gpt-5.5', enabled: true, priority: 0, builtin: true,
      }] };
      if (path === '/api/providers/catalog') throw new Error('catalog offline');
      throw new Error(`unexpected ${path}`);
    });

    await expect(refreshProviders()).resolves.toBeUndefined();
    expect(getProviderSnapshot().error).toBeUndefined();
    expect(getProviderSnapshot()).toMatchObject({
      providers: [{ id: 'codex-1', kind: 'openai-codex' }],
    });
  });

  it('uses the daemon auth and priority wire format', async () => {
    apiFetch.mockResolvedValue({
      success: true,
      data: { flowId: 'flow-1', state: 'pending', method: 'device_code', expiresAt: '2030-01-01T00:00:00Z' },
    });

    const flow = await startProviderAuth('provider-1', 'device');
    expect(flow).toMatchObject({ flowId: 'flow-1', status: 'pending', method: 'device' });
    expect(apiFetch).toHaveBeenNthCalledWith(1, '/api/providers/provider-1/auth/start', {
      method: 'POST', body: JSON.stringify({ method: 'device_code' }),
    });

    await completeProviderAuth('provider-1', 'flow-1', 'http://localhost/callback?code=x');
    expect(apiFetch).toHaveBeenNthCalledWith(2, '/api/providers/provider-1/auth/complete', {
      method: 'POST', body: JSON.stringify({ flowId: 'flow-1', input: 'http://localhost/callback?code=x' }),
    });

    apiFetch.mockImplementation(async (path: string) => path === '/api/providers/priorities'
      ? { success: true, data: [] }
      : { success: true, data: [] });
    await reorderProviderIds(['a', 'b']);
    expect(apiFetch).toHaveBeenCalledWith('/api/providers/priorities', {
      method: 'PUT', body: JSON.stringify({ providerIds: ['a', 'b'] }),
    });
  });

  it('updates the enabled model list and default model through the provider API', async () => {
    const models = [
      { id: 'grok-4.5', name: 'Grok 4.5', enabled: true },
      { id: 'grok-fast', name: 'Grok Fast', enabled: false },
    ];
    const imageModels = [{ id: 'grok-imagine', name: 'Grok Imagine', enabled: true }];
    apiFetch.mockImplementation(async (path: string) => {
      if (path === '/api/providers/provider-1') return { success: true, data: {} };
      if (path === '/api/providers') return { success: true, data: [] };
      if (path === '/api/providers/catalog') return { success: true, data: [] };
      throw new Error(`unexpected ${path}`);
    });

    await updateProviderModels('provider-1', models, imageModels, 'grok-4.5');

    expect(apiFetch).toHaveBeenNthCalledWith(1, '/api/providers/provider-1', {
      method: 'PATCH',
      body: JSON.stringify({ models, imageModels, defaultModel: 'grok-4.5' }),
    });
  });
});
