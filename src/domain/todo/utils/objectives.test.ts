import { describe, expect, it } from 'vitest';
import type { Goal, TodoItem } from '../types';
import { groupTodosByObjective, matchesTaskFilter } from './objectives';

const goal: Goal = {
  id: 'o1', title: 'Ship', status: 'active', createdAt: 'now', updatedAt: 'now',
  keyResults: [{ id: 'kr1', code: 'KR1', title: 'Release safely' }],
};

function todo(id: string, patch: Partial<TodoItem> = {}): TodoItem {
  return {
    id, title: id, type: 'today', completed: false, createdAt: 'now', order: 0, ...patch,
  };
}

describe('objective projections', () => {
  it('keeps unassigned work separate from objectives', () => {
    const groups = groupTodosByObjective([
      todo('task', { goalId: 'o1', keyResultId: 'kr1' }),
      todo('loose'),
    ], [goal], 'all');
    expect(groups.map(group => group.goal?.id ?? null)).toEqual(['o1', null]);
    expect(groups[0].keyResults[0].todos[0].id).toBe('task');
  });

  it('uses task state filters without changing the source data', () => {
    const active = todo('active', { status: 'in_progress' });
    const pending = todo('pending', { status: 'todo' });
    const done = todo('done', { completed: true, status: 'done' });
    expect(matchesTaskFilter(active, 'in_progress')).toBe(true);
    expect(matchesTaskFilter(pending, 'in_progress')).toBe(true);
    expect(matchesTaskFilter(done, 'in_progress')).toBe(false);
    expect(matchesTaskFilter(done, 'all')).toBe(true);
    expect(matchesTaskFilter(done, 'completed')).toBe(true);
    expect(groupTodosByObjective([active, done], [], 'completed')[0].keyResults[0].todos).toEqual([done]);
  });

  it('keeps tasks without a KR in an explicit bucket under their objective', () => {
    const groups = groupTodosByObjective([todo('loose', { goalId: 'o1' })], [goal], 'all');
    const unassigned = groups[0].keyResults[groups[0].keyResults.length - 1];
    expect(unassigned.keyResult).toBeNull();
    expect(unassigned.todos[0].id).toBe('loose');
  });
});
