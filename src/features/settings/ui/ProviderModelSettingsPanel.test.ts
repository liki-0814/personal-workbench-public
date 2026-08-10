import { describe, expect, it } from 'vitest';
import type { ModelEntry } from '@/core/config';
import {
  applyModelTokenLimits,
  applyModelVisionCapability,
  applyRestrictedToolSchemaCompatibility,
  applyThinkingLevelsToModels,
} from './providerModelThinking';

describe('applyThinkingLevelsToModels', () => {
  it('batch configures only selected models and preserves unrelated metadata', () => {
    const result = applyThinkingLevelsToModels([
      { id: 'a', name: 'A', contextWindow: 128_000 },
      { id: 'b', name: 'B', capabilities: { vision: true } },
    ], new Set(['a']), ['low', 'medium', 'high']);

    expect(result[0]).toMatchObject({
      id: 'a',
      contextWindow: 128_000,
      capabilities: { thinking: true },
      thinkingLevelMap: { low: 'low', medium: 'medium', high: 'high' },
    });
    expect(result[1]).toEqual({ id: 'b', name: 'B', capabilities: { vision: true } });
  });

  it('disables thinking when no strength is selected', () => {
    const [model] = applyThinkingLevelsToModels([{
      id: 'a',
      name: 'A',
      capabilities: { vision: true, thinking: true },
      thinkingLevelMap: { high: 'high' },
    }], new Set(['a']), []);

    expect(model.capabilities).toEqual({ vision: true, thinking: false });
    expect(model.thinkingLevelMap).toBeUndefined();
  });
});

describe('applyModelTokenLimits', () => {
  it('batch updates token limits while preserving unspecified values', () => {
    const models = [
      { id: 'a', name: 'A', contextWindow: 128_000, maxOutput: 8_192 },
      { id: 'b', name: 'B', contextWindow: 64_000 },
    ];

    expect(applyModelTokenLimits(models, new Set(['a']), { contextWindow: 256_000 })).toEqual([
      { id: 'a', name: 'A', contextWindow: 256_000, maxOutput: 8_192 },
      { id: 'b', name: 'B', contextWindow: 64_000 },
    ]);
  });
});

describe('applyModelVisionCapability', () => {
  it('batch toggles vision without dropping thinking metadata', () => {
    const models = [{ id: 'a', name: 'A', capabilities: { thinking: true } }, { id: 'b', name: 'B' }];
    expect(applyModelVisionCapability(models, new Set(['a']), true)).toEqual([
      { id: 'a', name: 'A', capabilities: { thinking: true, vision: true } },
      { id: 'b', name: 'B' },
    ]);
  });
});

describe('applyRestrictedToolSchemaCompatibility', () => {
  const models: ModelEntry[] = [
    { id: 'a', name: 'A', capabilities: { vision: true } },
    { id: 'b', name: 'B' },
  ];

  it('writes the restricted dialect capability only to selected models', () => {
    expect(applyRestrictedToolSchemaCompatibility(models, new Set(['a']), true)).toEqual([
      {
        id: 'a',
        name: 'A',
        capabilities: { vision: true, toolSchemaTopLevelCombinators: false },
      },
      models[1],
    ]);
  });

  it('restores full schema support explicitly', () => {
    expect(applyRestrictedToolSchemaCompatibility(models, new Set(['b']), false)[1].capabilities)
      .toEqual({ toolSchemaTopLevelCombinators: true });
  });
});
