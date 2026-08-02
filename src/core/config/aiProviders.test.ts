import { describe, expect, it } from 'vitest';
import {
  normalizeProviderConfig,
  normalizeProviderProtocol,
  supportsDeferredTools,
  supportsKimiDeferredTools,
  needsProxy,
  templatesForProtocol,
  validateProviderConfig,
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
    expect(templatesForProtocol('anthropic_messages').some(t => t.id === 'anthropic_thinking_budget')).toBe(true);
    expect(templatesForProtocol('openai_chat').some(t => t.id === 'openai_top_p')).toBe(true);
    expect(templatesForProtocol('openai_chat').some(t => t.id === 'anthropic_thinking_budget')).toBe(false);
  });

  it('applies template knobs without changing protocol code paths', () => {
    const template = templatesForProtocol('anthropic_messages').find(t => t.id === 'anthropic_thinking_budget')!;
    const model = template.apply({ id: 'claude', name: 'Claude' });
    expect(model.capabilities?.thinking).toBe(true);
    expect(model.thinkingParams?.budget_tokens).toBe(2048);
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
