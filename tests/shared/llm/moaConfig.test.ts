import { describe, expect, it } from 'vitest';
import { validateMoaConfig } from '../../../src/core/config/moaStore';

const models = [
  { id: 'advisor-a', name: 'Advisor A', provider: 'openai' as const, providerName: 'p', baseUrl: '', apiKey: '', providerIndex: 0 },
  { id: 'judge-a', name: 'Judge A', provider: 'openai' as const, providerName: 'p', baseUrl: '', apiKey: '', providerIndex: 0 },
];

describe('Harness MoA config', () => {
  it('validates active advisors and judge without creating virtual chat models', () => {
    expect(validateMoaConfig({
      activePreset: 'review',
      presets: {
        review: {
          enabled: true,
          advisors: [{ provider: 'p', model: 'advisor-a' }],
          judge: { provider: 'p', model: 'judge-a' },
        },
      },
    }, models)).toEqual([]);
  });

  it('rejects a missing active preset', () => {
    expect(validateMoaConfig({ activePreset: 'missing', presets: {} }, models))
      .toContain('当前 preset 不存在');
  });
});
