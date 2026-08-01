import { describe, expect, it } from 'vitest';

import { getModelMeta } from '@/core/config/modelMetadata';
import { getDefaultMaxOutput } from '@/core/llm';

describe('model metadata', () => {
  it('automatically configures bailian/glm-5.2 limits', () => {
    expect(getModelMeta('bailian/glm-5.2')).toEqual({
      contextWindow: 1_000_000,
      maxInput: 1_000_000,
      maxOutput: 131_072,
    });
    expect(getModelMeta('Bailian GLM 5.2')).toEqual(getModelMeta('bailian/glm-5.2'));
    expect(getDefaultMaxOutput('bailian/glm-5.2')).toBe(131_072);
  });
});
