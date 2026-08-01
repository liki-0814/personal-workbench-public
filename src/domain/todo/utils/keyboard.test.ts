import { describe, expect, it } from 'vitest';
import { shouldSubmitOnEnter } from './keyboard';

describe('shouldSubmitOnEnter', () => {
  it('does not submit while an input method is composing', () => {
    expect(shouldSubmitOnEnter('Enter', true)).toBe(false);
  });

  it('submits a normal Enter key press', () => {
    expect(shouldSubmitOnEnter('Enter', false)).toBe(true);
  });
});
