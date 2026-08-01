import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TodoItem } from '../../types';
import TaskDrawer from './TaskDrawer';

describe('TaskDrawer AI collaboration action', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it('starts AI with the latest draft and the bound execution workspace', async () => {
    const todo: TodoItem = {
      id: 'todo-1',
      title: '旧标题',
      type: 'today',
      completed: false,
      createdAt: '2026-07-30T00:00:00Z',
      order: 0,
      executionWorkspace: { path: '/workspace/project', boundAt: '2026-07-30T00:00:00Z' },
    };
    const onUpdate = vi.fn();
    const onStartAi = vi.fn();
    await act(async () => root.render(
      <TaskDrawer
        todo={todo}
        onClose={vi.fn()}
        onUpdate={onUpdate}
        onStartAi={onStartAi}
      />,
    ));

    const title = container.querySelector('[aria-label="任务标题"]') as HTMLInputElement;
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
      setter?.call(title, '刚刚修改的标题');
      title.dispatchEvent(new Event('input', { bubbles: true }));
    });
    const start = Array.from(container.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.textContent?.includes('AI 协作'))!;
    await act(async () => start.click());

    expect(onUpdate).toHaveBeenCalledWith('todo-1', expect.objectContaining({ title: '刚刚修改的标题' }));
    expect(onStartAi).toHaveBeenCalledWith(expect.objectContaining({
      id: 'todo-1',
      title: '刚刚修改的标题',
      executionWorkspace: todo.executionWorkspace,
    }));
  });
});
