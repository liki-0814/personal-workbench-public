import { describe, expect, it } from 'vitest';
import type { TodoItem } from '../types';
import { applyTodoProgress, getTodoProgress } from './progress';

const todo = (overrides: Partial<TodoItem> = {}): TodoItem => ({
  id: 'todo-1',
  title: '测试任务',
  type: 'today',
  completed: false,
  createdAt: '2026-07-27T00:00:00.000Z',
  order: 0,
  status: 'todo',
  ...overrides,
});

describe('todo progress', () => {
  it('normalizes legacy tasks and invalid values', () => {
    expect(getTodoProgress(todo())).toBe(0);
    expect(getTodoProgress(todo({ status: 'in_progress' }))).toBe(1);
    expect(getTodoProgress(todo({ completed: true }))).toBe(100);
    expect(getTodoProgress(todo({ progress: 37.6 }))).toBe(38);
    expect(getTodoProgress(todo({ progress: 120 }))).toBe(100);
  });

  it('keeps progress and task status in sync', () => {
    expect(applyTodoProgress(todo(), 0)).toMatchObject({
      progress: 0,
      status: 'todo',
      completed: false,
    });
    expect(applyTodoProgress(todo(), 45)).toMatchObject({
      progress: 45,
      status: 'in_progress',
      completed: false,
    });
    expect(applyTodoProgress(todo(), 100, '2026-07-27T01:00:00.000Z')).toMatchObject({
      progress: 100,
      status: 'done',
      completed: true,
      completedAt: '2026-07-27T01:00:00.000Z',
    });
  });

  it('clears completion metadata when progress drops below 100', () => {
    expect(applyTodoProgress(todo({
      completed: true,
      status: 'done',
      progress: 100,
      completedAt: '2026-07-27T01:00:00.000Z',
    }), 60)).toMatchObject({
      progress: 60,
      status: 'in_progress',
      completed: false,
      completedAt: undefined,
    });
  });
});
