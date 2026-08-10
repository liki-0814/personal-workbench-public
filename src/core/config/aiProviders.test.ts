import { describe, expect, it } from 'vitest';
import {
  normalizeProviderConfig,
  normalizeProviderProtocol,
  supportsDeferredTools,
  supportsKimiDeferredTools,
  supportedThinkingLevelsForModel,
  preferredThinkingLevel,
  needsProxy,
  templatesForProtocol,
  validateProviderConfig,
  getModelInfo,
  modelSelectionKey,
  resolveModelSelection,
  setProviders,
  type AiProvider,
} from './aiProviders';

function provider(
  name: string,
  baseUrl: string,
  extras: Partial<AiProvider> = {},
): AiProvider {
  return {
    name,
    baseUrl,
    apiKey: 'masked',
    protocol: 'openai_chat',
    models: [],
    ...extras,
  };
}

describe('supportsDeferredTools', () => {
  it('is driven only by deferredToolsMode config', () => {
    expect(supportsDeferredTools(provider('Example', 'https://api.example.com/v1'))).toBe(false);
    expect(supportsDeferredTools(provider('Example', 'https://api.example.com/v1', {
      models: [{ id: 'm', name: 'm', deferredToolsMode: 'enabled' }],
    }))).toBe(true);
    expect(supportsKimiDeferredTools(provider('Example', 'https://api.example.com/v1', {
      models: [{ id: 'm', name: 'm', deferredToolsMode: 'enabled' }],
    }))).toBe(true);
  });
});

describe('provider-scoped model identity', () => {
  it('routes duplicate model ids only when the provider is explicit', () => {
    setProviders([
      provider('Provider A', 'https://a.example/v1', {
        id: 'provider-a',
        models: [{ id: 'shared-model', name: 'Shared Model' }],
      }),
      provider('Provider B', 'https://b.example/v1', {
        id: 'provider-b',
        models: [{ id: 'shared-model', name: 'Shared Model' }],
      }),
    ]);
    try {
      const key = modelSelectionKey({ id: 'shared-model', providerId: 'provider-b' });
      expect(key).toBe('provider:provider-b/shared-model');
      expect(resolveModelSelection(key)).toBe(key);
      expect(getModelInfo(key)).toMatchObject({ providerId: 'provider-b', baseUrl: 'https://b.example/v1' });
      expect(getModelInfo('shared-model')).toMatchObject({ id: '', name: '请选择具体服务商' });
    } finally {
      setProviders([]);
    }
  });
});

describe('thinking levels', () => {
  it('keeps Pi ordering and hides levels the model marks unsupported', () => {
    const levels = supportedThinkingLevelsForModel({
      capabilities: { thinking: true },
      thinkingLevelMap: { low: 'low', medium: null, high: 'high', xhigh: 'xhigh' },
    });
    expect(levels).toEqual(['off', 'low', 'high', 'xhigh']);
    expect(preferredThinkingLevel(levels)).toBe('high');
  });

  it('falls back to off and medium when a thinking model has no level metadata', () => {
    expect(supportedThinkingLevelsForModel({ capabilities: { thinking: true } })).toEqual(['off', 'medium']);
  });
});

describe('needsProxy', () => {
  it('is driven only by explicit useProxy flag', () => {
    expect(needsProxy(provider('kimi', 'https://api.kimi.com/coding/v1'))).toBe(false);
    expect(needsProxy(provider('kimi', 'https://api.kimi.com/coding/v1', { useProxy: true }))).toBe(true);
  });
});

describe('normalizeProviderProtocol', () => {
  it('maps legacy and canonical protocol names', () => {
    expect(normalizeProviderProtocol('openai')).toBe('openai_chat');
    expect(normalizeProviderProtocol('anthropic')).toBe('anthropic_messages');
    expect(normalizeProviderProtocol('openai-responses')).toBe('openai_responses');
    expect(normalizeProviderProtocol('openai-codex-responses')).toBe('openai_responses');
    expect(normalizeProviderProtocol('google-generative')).toBe('google_generative');
  });
});

describe('normalizeProviderConfig', () => {
  it('normalizes legacy protocol names and deferredToolsMode values only', () => {
    const normalized = normalizeProviderConfig({
      name: 'gateway',
      baseUrl: 'https://gateway.example.com/v1',
      apiKey: 'sk',
      protocol: 'openai',
      models: [{ id: 'demo', name: 'Demo', deferredToolsMode: 'legacy-on' }],
    });
    expect(normalized.protocol).toBe('openai_chat');
    expect(normalized.useProxy).toBeUndefined();
    expect(normalized.models[0].deferredToolsMode).toBe('enabled');
  });

  it('does not infer useProxy from provider name or baseUrl', () => {
    const normalized = normalizeProviderConfig({
      name: 'kimi',
      baseUrl: 'https://api.kimi.com/coding/v1',
      apiKey: 'sk',
      protocol: 'openai_chat',
      models: [],
    });
    expect(normalized.useProxy).toBeUndefined();
  });
});

describe('templatesForProtocol', () => {
  it('returns protocol-specific templates only', () => {
    expect(templatesForProtocol('anthropic_messages').some(t => t.id === 'anthropic_adaptive_thinking')).toBe(true);
    expect(templatesForProtocol('openai_chat').some(t => t.id === 'openai_top_p')).toBe(true);
    expect(templatesForProtocol('openai_chat').some(t => t.id === 'anthropic_adaptive_thinking')).toBe(false);
  });

  it('applies template knobs without changing protocol code paths', () => {
    const template = templatesForProtocol('anthropic_messages').find(t => t.id === 'anthropic_adaptive_thinking')!;
    const model = template.apply({ id: 'claude', name: 'Claude' });
    expect(model.capabilities?.thinking).toBe(true);
    expect(model.thinkingParams?.thinking).toEqual({ type: 'adaptive' });
    expect(model.thinkingParams?.output_config).toEqual({ effort: 'medium' });
  });
});

describe('validateProviderConfig', () => {
  it('soft-warns on cross-protocol knobs', () => {
    const warnings = validateProviderConfig({
      name: 'x',
      baseUrl: 'https://example.com/v1',
      apiKey: 'sk',
      protocol: 'openai_chat',
      models: [{
        id: 'm',
        name: 'm',
        thinkingParams: { budget_tokens: 2048 },
      }],
    });
    expect(warnings.length).toBeGreaterThan(0);
  });
});
