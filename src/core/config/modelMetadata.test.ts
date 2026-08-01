import { describe, expect, it } from 'vitest';

import { getModelMeta } from './modelMetadata';

describe('modelMetadata', () => {
  it('loads newly added models from the shared CSV', () => {
    expect(getModelMeta('Peach-07-17-DogFooding')).toEqual({
      contextWindow: 990_998,
      maxInput: 991_000,
      maxOutput: 65_536,
    });
  });
});
