import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as studioApi from '../../api';
import FilePromptPicker from './FilePromptPicker';

const configMock = vi.hoisted(() => ({ showHidden: false }));

vi.mock('../../api');
vi.mock('@/core/utils/showHiddenFiles', () => ({
  useShowHiddenFiles: () => configMock.showHidden,
  filterHidden: <T extends { name: string }>(entries: T[], showHidden: boolean) =>
    showHidden ? entries : entries.filter(entry => !entry.name.startsWith('.')),
}));

describe('FilePromptPicker', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    configMock.showHidden = false;
    vi.mocked(studioApi.listDirectory).mockResolvedValue([
      { name: '.private.md', type: 'file' },
      { name: 'review.md', type: 'file' },
    ]);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.resetAllMocks();
  });

  const renderPicker = async () => {
    await act(async () => root.render(
      <FilePromptPicker
        query=">"
        promptsDir="/workspace/prompts"
        onPick={vi.fn()}
        onClose={vi.fn()}
      />,
    ));
    await act(async () => Promise.resolve());
  };

  it('根据设置即时隐藏或显示隐藏指令文件', async () => {
    await renderPicker();
    expect(container.textContent).toContain('review');
    expect(container.textContent).not.toContain('.private');

    configMock.showHidden = true;
    await renderPicker();

    expect(container.textContent).toContain('.private');
  });
});
