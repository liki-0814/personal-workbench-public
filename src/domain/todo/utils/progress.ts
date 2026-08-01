import type { TodoItem } from '../types';

export function getTodoProgress(todo: TodoItem): number {
  if (todo.completed || todo.status === 'done') return 100;
  if (typeof todo.progress !== 'number' || !Number.isFinite(todo.progress)) {
    return todo.status === 'in_progress' ? 1 : 0;
  }
  return Math.min(100, Math.max(0, Math.round(todo.progress)));
}

export function applyTodoProgress(
  todo: TodoItem,
  value: number,
  completedAt?: string,
): TodoItem {
  const progress = Math.min(100, Math.max(0, Math.round(value)));

  if (progress === 100) {
    return {
      ...todo,
      progress,
      status: 'done',
      completed: true,
      completedAt: todo.completedAt ?? completedAt,
    };
  }

  return {
    ...todo,
    progress,
    status: progress === 0 ? 'todo' : 'in_progress',
    completed: false,
    completedAt: undefined,
  };
}
