import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { apiFetch } from '@/core/utils';
import DirectoryPickerModal, { type ResolvedDirectory } from './DirectoryPickerModal';

const configMock = vi.hoisted(() => ({ fsBase: '', showHidden: false }));

vi.mock('@/core/utils', () => ({
  apiFetch: vi.fn(),
}));

vi.mock('@/core/config', () => ({
  normalizeFsBase: (value?: string) => value?.trim() || '~/',
  useLocalConfig: () => ({ tools: { fsBase: configMock.fsBase } }),
}));

vi.mock('@/core/utils/showHiddenFiles', () => ({
  useShowHiddenFiles: () => configMock.showHidden,
  filterHidden: <T extends { name: string }>(entries: T[], showHidden: boolean) =>
    showHidden ? entries : entries.filter(entry => !entry.name.startsWith('.')),
}));

const apiFetchMock = vi.mocked(apiFetch);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const resolvedDirectory: ResolvedDirectory = {
  canonicalPath: '/workspace/project',
  displayPath: '~/project',
  fsBase: '/workspace',
  readable: true,
};

describe('DirectoryPickerModal', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    apiFetchMock.mockReset();
    configMock.fsBase = '';
    configMock.showHidden = false;
    apiFetchMock.mockImplementation(async path => {
      if (path.startsWith('/api/fs/list')) {
        return {
          success: true,
          path: '/workspace/project',
          entries: [],
        };
      }
      throw new Error(`Unexpected request: ${path}`);
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  const renderPicker = async (
    onClose = vi.fn(),
    onConfirm = vi.fn(),
  ) => {
    await act(async () => root.render(
      <DirectoryPickerModal
        open
        projects={[]}
        initialPath="/workspace/project"
        onClose={onClose}
        onConfirm={onConfirm}
      />,
    ));
    await act(async () => Promise.resolve());
    return { onClose, onConfirm };
  };

  const confirmButton = () => Array.from(document.body.querySelectorAll<HTMLButtonElement>('button'))
    .find(button => button.textContent?.includes('使用此文件夹')) as HTMLButtonElement;

  it.each([
    ['取消', () => Array.from(document.body.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.textContent === '取消')?.click()],
    ['关闭按钮', () => document.body.querySelector<HTMLButtonElement>('[aria-label="关闭文件夹选择"]')?.click()],
    ['遮罩', () => document.body.querySelector<HTMLElement>('.workspace-picker-backdrop')
      ?.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))],
  ])('确认请求期间通过%s关闭后不创建会话', async (_label, closePicker) => {
    const resolveRequest = deferred<ResolvedDirectory>();
    apiFetchMock.mockImplementation(path => {
      if (path.startsWith('/api/fs/list')) {
        return Promise.resolve({ success: true, path: '/workspace/project', entries: [] });
      }
      return resolveRequest.promise;
    });
    const { onClose, onConfirm } = await renderPicker();

    await act(async () => confirmButton().click());
    const resolveCall = apiFetchMock.mock.calls.find(([path]) => path === '/api/fs/resolve-directory');
    expect(resolveCall?.[1]?.signal).toBeInstanceOf(AbortSignal);

    await act(async () => closePicker());
    expect((resolveCall?.[1]?.signal as AbortSignal).aborted).toBe(true);

    await act(async () => resolveRequest.resolve(resolvedDirectory));
    expect(onClose).toHaveBeenCalledOnce();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it('校验失败时保留弹窗和错误原因', async () => {
    apiFetchMock.mockImplementation(path => {
      if (path.startsWith('/api/fs/list')) {
        return Promise.resolve({ success: true, path: '/workspace/project', entries: [] });
      }
      return Promise.reject(new Error('目录不可读'));
    });
    const { onClose, onConfirm } = await renderPicker();

    await act(async () => {
      confirmButton().click();
      await Promise.resolve();
    });

    expect(document.body.querySelector('[role="dialog"]')).not.toBeNull();
    expect(document.body.querySelector('[role="alert"]')?.textContent).toContain('目录不可读');
    expect(confirmButton().disabled).toBe(false);
    expect(onClose).not.toHaveBeenCalled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it('连续点击确认只发起一次校验和创建', async () => {
    const resolveRequest = deferred<ResolvedDirectory>();
    apiFetchMock.mockImplementation(path => {
      if (path.startsWith('/api/fs/list')) {
        return Promise.resolve({ success: true, path: '/workspace/project', entries: [] });
      }
      return resolveRequest.promise;
    });
    const { onConfirm } = await renderPicker();

    await act(async () => {
      confirmButton().click();
      confirmButton().click();
    });

    expect(apiFetchMock.mock.calls.filter(([path]) => path === '/api/fs/resolve-directory')).toHaveLength(1);
    await act(async () => resolveRequest.resolve(resolvedDirectory));
    expect(onConfirm).toHaveBeenCalledOnce();
    expect(onConfirm).toHaveBeenCalledWith(resolvedDirectory);
  });

  it('使用显式默认根目录并在配置投影变化后立即刷新', async () => {
    await act(async () => root.render(
      <DirectoryPickerModal
        open
        projects={[]}
        onClose={vi.fn()}
        onConfirm={vi.fn()}
      />,
    ));
    await act(async () => Promise.resolve());

    expect(apiFetchMock).toHaveBeenCalledWith('/api/fs/list?path=~%2F');

    configMock.fsBase = '/workspace/old-root';
    await act(async () => root.render(
      <DirectoryPickerModal
        open
        projects={[]}
        initialPath="/workspace/old-root/project"
        onClose={vi.fn()}
        onConfirm={vi.fn()}
      />,
    ));
    await act(async () => Promise.resolve());

    configMock.fsBase = '/workspace/new-root';
    await act(async () => root.render(
      <DirectoryPickerModal
        open
        projects={[]}
        initialPath="/workspace/old-root/project"
        onClose={vi.fn()}
        onConfirm={vi.fn()}
      />,
    ));
    await act(async () => Promise.resolve());

    expect(apiFetchMock).toHaveBeenCalledWith('/api/fs/list?path=%2Fworkspace%2Fnew-root');
  });

  it('根据设置即时隐藏或显示隐藏文件夹', async () => {
    apiFetchMock.mockResolvedValue({
      success: true,
      path: '/workspace/project',
      entries: [
        { name: '.git', type: 'directory' },
        { name: 'src', type: 'directory' },
      ],
    });

    await renderPicker();
    expect(document.body.textContent).toContain('src');
    expect(document.body.textContent).not.toContain('.git');

    configMock.showHidden = true;
    await act(async () => root.render(
      <DirectoryPickerModal
        open
        projects={[]}
        initialPath="/workspace/project"
        onClose={vi.fn()}
        onConfirm={vi.fn()}
      />,
    ));

    expect(document.body.textContent).toContain('.git');
  });
});
