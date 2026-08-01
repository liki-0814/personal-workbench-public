import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as studioApi from '../../api';
import FileTree from './FileTree';

vi.mock('../../api');
vi.mock('@/shell', () => ({ showToast: vi.fn() }));

describe('FileTree', () => {
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
    vi.resetAllMocks();
  });

  it('renders a real empty directory separately from a loading failure', async () => {
    vi.mocked(studioApi.listDirectory).mockResolvedValue([]);
    await act(async () => root.render(<FileTree rootDir="/workspace" onOpenFile={() => {}} />));
    await act(async () => {});
    expect(container.textContent).toContain('文件夹为空');
    expect(container.textContent).not.toContain('重试');
  });

  it('shows a retry action and does not claim the directory is empty on failure', async () => {
    vi.mocked(studioApi.listDirectory).mockRejectedValue(new Error('Access denied'));
    await act(async () => root.render(<FileTree rootDir="/outside" onOpenFile={() => {}} />));
    await act(async () => {});
    expect(container.textContent).toContain('Access denied');
    expect(container.textContent).toContain('重试');
    expect(container.textContent).not.toContain('文件夹为空');
  });
});
