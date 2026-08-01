import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { classifyTodo, groupTodos, formatRelativeDate } from '@/domain/todo/utils/grouping';
import { makeTodo } from '../fixtures/workbench';

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 5, 20, 10, 0, 0)); // 2026-06-20 Saturday
});

afterEach(() => {
  vi.useRealTimers();
});

describe('classifyTodo', () => {
  it('classifies completed todos', () => {
    expect(classifyTodo(makeTodo({ completed: true, dueDate: '2026-06-20' }))).toBe('completed');
  });

  it('classifies overdue todos', () => {
    expect(classifyTodo(makeTodo({ dueDate: '2026-06-18' }))).toBe('overdue');
  });

  it('classifies today todos by dueDate', () => {
    expect(classifyTodo(makeTodo({ dueDate: '2026-06-20' }))).toBe('today');
  });

  it('classifies this-week todos', () => {
    // 2026-06-21 is Sunday, same week
    expect(classifyTodo(makeTodo({ dueDate: '2026-06-21' }))).toBe('thisweek');
  });

  it('classifies later todos', () => {
    expect(classifyTodo(makeTodo({ dueDate: '2026-07-15' }))).toBe('later');
  });

  it('classifies type=today without dueDate as today', () => {
    expect(classifyTodo(makeTodo({ type: 'today' }))).toBe('today');
  });

  it('classifies type=week without dueDate as thisweek', () => {
    expect(classifyTodo(makeTodo({ type: 'week' }))).toBe('thisweek');
  });

  it('classifies no dueDate + longterm type as no_date', () => {
    expect(classifyTodo(makeTodo({ type: 'longterm' }))).toBe('no_date');
  });
});

describe('groupTodos', () => {
  it('groups todos into ordered buckets', () => {
    const todos = [
      makeTodo({ id: '1', dueDate: '2026-06-18' }), // overdue
      makeTodo({ id: '2', dueDate: '2026-06-20' }), // today
      makeTodo({ id: '3', type: 'longterm' }),        // no_date
      makeTodo({ id: '4', completed: true }),          // completed
    ];
    const groups = groupTodos(todos);
    expect(groups.map(g => g.meta.key)).toEqual(['overdue', 'today', 'no_date', 'completed']);
    expect(groups[0].items[0].id).toBe('1');
    expect(groups[1].items[0].id).toBe('2');
  });

  it('omits empty groups', () => {
    const todos = [makeTodo({ dueDate: '2026-06-20' })];
    const groups = groupTodos(todos);
    expect(groups).toHaveLength(1);
    expect(groups[0].meta.key).toBe('today');
  });

  it('returns empty array for no todos', () => {
    expect(groupTodos([])).toEqual([]);
  });
});

describe('formatRelativeDate', () => {
  it('returns empty for no date', () => {
    expect(formatRelativeDate('')).toEqual({ label: '', tone: 'later' });
  });

  it('shows overdue days', () => {
    const result = formatRelativeDate('2026-06-17');
    expect(result.label).toBe('逾期 3 天');
    expect(result.tone).toBe('overdue');
  });

  it('shows 今天 for today', () => {
    expect(formatRelativeDate('2026-06-20')).toEqual({ label: '今天', tone: 'today' });
  });

  it('shows 明天 for tomorrow', () => {
    expect(formatRelativeDate('2026-06-21')).toEqual({ label: '明天', tone: 'soon' });
  });

  it('shows 后天 for day after tomorrow', () => {
    expect(formatRelativeDate('2026-06-22')).toEqual({ label: '后天', tone: 'soon' });
  });

  it('shows weekday for 3-6 days ahead', () => {
    // 2026-06-23 is Tuesday
    const result = formatRelativeDate('2026-06-23');
    expect(result.label).toBe('周二');
    expect(result.tone).toBe('soon');
  });

  it('shows month/day for 7+ days ahead', () => {
    const result = formatRelativeDate('2026-06-30');
    expect(result.label).toBe('6月30日');
    expect(result.tone).toBe('later');
  });
});
