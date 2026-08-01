import { describe, expect, it } from 'vitest';
import type { ChatSession } from '../types';
import type { TodoItem } from '@/domain/todo';
import { resolveExecutionWorkspace } from './executionWorkspace';

const session = (patch: Partial<ChatSession> = {}): ChatSession => ({
  id: 'session-1',
  title: 'task',
  messages: [],
  model: 'test',
  createdAt: '2026-07-18T00:00:00Z',
  updatedAt: '2026-07-18T00:00:00Z',
  ...patch,
});

const todo = (patch: Partial<TodoItem> = {}): TodoItem => ({
  id: 'todo-1',
  title: 'task',
  type: 'today',
  completed: false,
  createdAt: '2026-07-18T00:00:00Z',
  order: 0,
  ...patch,
});

describe('resolveExecutionWorkspace', () => {
  it('prefers the linked task workspace over legacy session cwd', () => {
    expect(resolveExecutionWorkspace(
      session({ taskId: 'todo-1', cwd: '/legacy' }),
      [todo({ executionWorkspace: { path: '/task/project', boundAt: '2026-07-18T00:00:00Z' } })],
    )).toBe('/task/project');
  });

  it('keeps legacy cwd as read-only compatibility', () => {
    expect(resolveExecutionWorkspace(session({ cwd: '/legacy' }), [])).toBe('/legacy');
  });

  it('does not invent a workspace', () => {
    expect(resolveExecutionWorkspace(session({ taskId: 'todo-1' }), [todo()])).toBeUndefined();
  });
});
