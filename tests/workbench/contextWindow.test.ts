import { describe, expect, it } from 'vitest';
import {
  estimateDraftTokens,
  estimateVisibleContextTokens,
  formatTokenAmount,
  getContextUsagePercent,
  getContextWindow,
} from '@/domain/chat/utils/contextWindow';

describe('context window usage', () => {
  it('estimates visible messages without counting UI-only thinking traces', () => {
    const plain = estimateVisibleContextTokens([{ role: 'user', content: '你好' }]);
    const withThinking = estimateVisibleContextTokens([{
      role: 'user',
      content: '你好',
      thinking: '这段不会被重新发送给模型',
    }]);
    expect(withThinking).toBe(plain);
    expect(plain).toBeGreaterThan(2);
  });

  it('adds current input, attachment content, and images', () => {
    const withoutExtras = estimateDraftTokens('hello');
    const withExtras = estimateDraftTokens('hello', ['data:image/png;base64,x'], [{
      type: 'file', id: '1', title: 'a.ts', content: 'const answer = 42;',
    }]);
    expect(withExtras).toBeGreaterThan(withoutExtras + 1_000);
  });

  it('uses exact metadata and a conservative fallback', () => {
    expect(getContextWindow('claude-opus-4-6')).toEqual({
      windowTokens: 1_000_000,
      exactWindow: true,
    });
    expect(getContextWindow('future-model')).toEqual({
      windowTokens: 32_000,
      exactWindow: false,
    });
  });

  it('formats token magnitudes and percentages', () => {
    expect(formatTokenAmount(12_400)).toBe('12.4K');
    expect(formatTokenAmount(1_000_000)).toBe('1M');
    expect(getContextUsagePercent(12_400, 1_000_000)).toBeCloseTo(1.24);
  });
});
