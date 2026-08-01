import { describe, expect, it } from 'vitest';
import { parseWorkbenchResponse } from './parseWorkbenchInput';

describe('parseWorkbenchResponse', () => {
  it('parses an objective with key results', () => {
    expect(parseWorkbenchResponse('{"intent":"objective","title":"提升效率","keyResults":[{"title":"主要操作三步内完成"}]}')).toEqual({
      intent: 'objective',
      title: '提升效率',
      description: undefined,
      keyResults: [{ title: '主要操作三步内完成' }],
    });
  });

  it('parses key results for an existing objective', () => {
    expect(parseWorkbenchResponse('{"intent":"key_result","objectiveTitle":"计费调控","keyResults":[{"title":"完成核心链路验收"}]}')).toEqual({
      intent: 'key_result',
      objectiveTitle: '计费调控',
      keyResults: [{ title: '完成核心链路验收' }],
    });
  });

  it('falls back to a task for an omitted intent', () => {
    expect(parseWorkbenchResponse('{"title":"整理周报","priority":"low","subTasks":[]}').intent).toBe('task');
  });

  it('keeps the objective and key result recognized for a task', () => {
    expect(parseWorkbenchResponse('{"intent":"task","title":"验证模型","objectiveTitle":"计费调控","keyResultTitle":"KR1","subTasks":[]}')).toMatchObject({
      intent: 'task',
      objectiveTitle: '计费调控',
      keyResultTitle: 'KR1',
    });
  });
});
