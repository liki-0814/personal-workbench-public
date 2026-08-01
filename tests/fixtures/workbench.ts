import type { TodoItem } from '@/types';

export function makeTodo(overrides: Partial<TodoItem> = {}): TodoItem {
  return {
    id: `todo-${Math.random().toString(36).slice(2, 7)}`,
    title: 'Test Task',
    type: 'today',
    completed: false,
    createdAt: '2024-01-01T00:00:00Z',
    order: 0,
    ...overrides,
  };
}
