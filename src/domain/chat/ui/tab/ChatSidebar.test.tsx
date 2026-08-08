import { act, type ComponentProps } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { LocalProject } from '@/domain/chat';
import ChatSidebar from './ChatSidebar';

const project: LocalProject = {
  id: 'project-1',
  name: 'personal-workbench-public',
  canonicalPath: '/workspace/personal-workbench-public',
  displayPath: '/workspace/personal-workbench-public',
  availability: 'ready',
  createdAt: '2026-08-09T00:00:00Z',
  lastOpenedAt: '2026-08-09T00:00:00Z',
};

describe('ChatSidebar project actions', () => {
  let container: HTMLDivElement;
  let root: Root;
  const writeText = vi.fn(() => Promise.resolve());

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    writeText.mockClear();
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  const renderSidebar = async (overrides: Partial<ComponentProps<typeof ChatSidebar>> = {}) => {
    const props: ComponentProps<typeof ChatSidebar> = {
      sessions: [],
      projects: [project],
      activeSessionId: null,
      onSelectSession: vi.fn(),
      onCreateSession: vi.fn(),
      onDeleteSession: vi.fn(),
      onUpdateSession: vi.fn(),
      onRenameProject: vi.fn(),
      onSetProjectPinned: vi.fn(),
      onRemoveProject: vi.fn(),
      ...overrides,
    };
    await act(async () => root.render(<ChatSidebar {...props} />));
    return props;
  };

  const button = (label: string) => container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!;

  it('项目加号直接传递已绑定路径', async () => {
    const props = await renderSidebar();
    await act(async () => button(`在 ${project.name} 新建会话`).click());
    expect(props.onCreateSession).toHaveBeenCalledWith(project.canonicalPath);
  });

  it('项目菜单支持置顶、编辑名称和复制路径', async () => {
    const props = await renderSidebar();

    await act(async () => button(`${project.name} 更多操作`).click());
    await act(async () => Array.from(container.querySelectorAll('button')).find(item => item.textContent === '置顶项目')!.click());
    expect(props.onSetProjectPinned).toHaveBeenCalledWith(project.id, true);

    await act(async () => button(`${project.name} 更多操作`).click());
    await act(async () => Array.from(container.querySelectorAll('button')).find(item => item.textContent === '编辑项目名称')!.click());
    const input = container.querySelector<HTMLInputElement>(`[aria-label="编辑项目名称：${project.name}"]`)!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set?.call(input, 'pwcli');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await act(async () => button('确认项目名称').click());
    expect(props.onRenameProject).toHaveBeenCalledWith(project.id, 'pwcli');

    await act(async () => button(`${project.name} 更多操作`).click());
    await act(async () => Array.from(container.querySelectorAll('button')).find(item => item.textContent === '复制项目路径')!.click());
    expect(writeText).toHaveBeenCalledWith(project.canonicalPath);
  });

  it('项目菜单复用统一键盘导航', async () => {
    await renderSidebar();
    await act(async () => button(`${project.name} 更多操作`).click());
    expect(document.activeElement?.textContent).toBe('置顶项目');
    await act(async () => document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'ArrowDown',
      bubbles: true,
    })));
    expect(document.activeElement?.textContent).toBe('编辑项目名称');
    await act(async () => document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'Escape',
      bubbles: true,
    })));
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });
});
