import { describe, expect, it } from 'vitest';
import type { Goal } from '../types';
import { resolveAiTarget } from './resolveAiTarget';

const goals: Goal[] = [
  {
    id: 'goal-1',
    title: '计费调控',
    status: 'active',
    createdAt: '2026-01-01',
    updatedAt: '2026-01-01',
    keyResults: [
      { id: 'kr-1', code: 'KR1', title: '建设计费模型化能力' },
      { id: 'kr-2', code: 'KR2', title: '提升低成本广告竞争力' },
    ],
  },
];

describe('resolveAiTarget', () => {
  it('matches an existing objective and key result', () => {
    const result = resolveAiTarget(goals, '计费调控', 'KR1');
    expect(result.goal?.id).toBe('goal-1');
    expect(result.keyResult?.id).toBe('kr-1');
  });

  it('keeps the objective when the key result is ambiguous or missing', () => {
    const result = resolveAiTarget(goals, '计费调控', '不存在');
    expect(result.goal?.id).toBe('goal-1');
    expect(result.keyResult).toBeUndefined();
  });

  it('does not guess an unknown objective', () => {
    expect(resolveAiTarget(goals, '其他目标')).toEqual({});
  });
});
