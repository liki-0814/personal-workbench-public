import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { readRuntimeTaskReviewDiff, readRuntimeTaskReviewFile } from '../../api';
import StudioTab from './StudioTab';

const studio = {
  rootDir: '/project',
  setRootDir: vi.fn(),
  tabs: [],
  activeTab: null,
  activeTabId: null,
  setActiveTabId: vi.fn(),
  openFile: vi.fn(),
  closeTab: vi.fn(),
  setContent: vi.fn(),
  saveFile: vi.fn(),
};

vi.mock('../../api', () => ({
  readRuntimeTaskReviewDiff: vi.fn(),
  readRuntimeTaskReviewFile: vi.fn(),
}));
vi.mock('../../state/store', () => ({
  detectLanguage: () => 'typescript',
  useStudio: () => studio,
}));
vi.mock('../components/FileTree', () => ({ default: () => <div /> }));
vi.mock('../components/FileTabs', () => ({ default: () => <div /> }));
vi.mock('../components/DiffPane', () => ({ default: () => <div>diff</div> }));
vi.mock('../components/EditorPane', () => ({
  default: ({ content, readOnly }: { content: string; readOnly?: boolean }) => (
    <pre data-readonly={String(readOnly)}>{content}</pre>
  ),
}));

describe('StudioTab RuntimeTask review file', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    vi.mocked(readRuntimeTaskReviewFile).mockReset();
    vi.mocked(readRuntimeTaskReviewDiff).mockReset();
    vi.mocked(readRuntimeTaskReviewDiff).mockResolvedValue({
      taskId: 'task-1',
      reviewRevision: 3,
      patchSha256: 'sha256',
      baselineCommit: 'baseline',
      historyAnomaly: false,
      status: 'pending',
      files: [{
        path: 'src/App.tsx',
        status: 'modified',
        size: 18,
        binary: false,
        truncated: false,
        patch: '@@ canonical @@',
      }],
      previewTruncated: false,
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it('loads the worker revision through the task endpoint and renders it read-only', async () => {
    vi.mocked(readRuntimeTaskReviewFile).mockResolvedValue({
      taskId: 'task-1',
      reviewRevision: 3,
      path: 'src/App.tsx',
      content: 'worker latest',
      size: 13,
      isBinary: false,
    });
    await act(async () => root.render(
      <StudioTab
        isDark={false}
        workspaceRef={{
          kind: 'diff',
          taskId: 'task-1',
          path: 'src/App.tsx',
          workspaceRoot: '/project',
          reviewRevision: 3,
          inlinePatch: '@@ -1 +1 @@',
        }}
      />,
    ));

    const fileButton = Array.from(container.querySelectorAll('button'))
      .find(button => button.textContent === '文件');
    await act(async () => fileButton?.click());

    expect(readRuntimeTaskReviewFile).toHaveBeenCalledWith(
      'task-1',
      'src/App.tsx',
      expect.any(AbortSignal),
    );
    expect(container.querySelector('pre')?.textContent).toBe('worker latest');
    expect(container.querySelector('pre')?.getAttribute('data-readonly')).toBe('true');
    expect(studio.openFile).not.toHaveBeenCalled();
  });

  it('loads the canonical frozen diff even when a card supplied an inline preview', async () => {
    await act(async () => root.render(
      <StudioTab
        isDark={false}
        workspaceRef={{
          kind: 'diff',
          taskId: 'task-1',
          path: 'src/App.tsx',
          workspaceRoot: '/project',
          reviewRevision: 3,
          inlinePatch: '@@ preview @@',
        }}
      />,
    ));

    expect(readRuntimeTaskReviewDiff).toHaveBeenCalledWith(
      'task-1',
      'src/App.tsx',
      3,
      expect.any(AbortSignal),
    );
  });
});
