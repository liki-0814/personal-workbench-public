import type { TodoItem } from '@/domain/todo';
import type { ChatSession } from '../types';

export function resolveExecutionWorkspace(session: ChatSession | null, todos: TodoItem[]): string | undefined {
  if (!session) return undefined;
  const taskWorkspace = session.taskId
    ? todos.find(todo => todo.id === session.taskId)?.executionWorkspace?.path.trim()
    : '';
  return taskWorkspace || session.cwd?.trim() || undefined;
}
