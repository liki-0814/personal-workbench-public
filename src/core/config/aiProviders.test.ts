import { describe, expect, it } from 'vitest';
import { supportsKimiDeferredTools, type AiProvider } from './aiProviders';

function provider(name: string, baseUrl: string): AiProvider {
  return { name, baseUrl, apiKey: 'masked', protocol: 'openai', models: [] };
}

describe('supportsKimiDeferredTools', () => {
  it('only exposes the Kimi-specific control for Kimi-compatible providers', () => {
    expect(supportsKimiDeferredTools(provider('kimi', 'https://api.kimi.com/coding/v1'))).toBe(true);
    expect(supportsKimiDeferredTools(provider('Moonshot', 'https://api.moonshot.cn/v1'))).toBe(true);
    expect(supportsKimiDeferredTools(provider('Example', 'https://api.example.com/v1'))).toBe(false);
  });
});
