import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RuntimeTask } from '../../state/taskRuntimeStore';
import CollaborationPanorama from './CollaborationPanorama';

const task: RuntimeTask = {
  id: 'task-1',
  batchId: 'batch-1',
  rootSessionId: 'session-1',
  kind: 'delegated',
  objective: '实现任务闭环',
  deliverableTitle: '任务闭环实现报告',
  cwd: '/workspace',
  status: 'succeeded',
  deliveryStatus: 'delivered',
  attempt: 1,
  createdAt: '2026-07-30T01:00:00.000Z',
  updatedAt: '2026-07-30T01:01:00.000Z',
  role: 'engineer',
  roleLabel: '工程师',
  displayName: 'Mia',
  resolvedExecutorId: 'codex',
  primaryDocumentId: 'doc-1',
  outputStatus: 'ready',
  result: { changedFiles: [{ path: 'src/App.tsx', patch: '@@ -1 +1 @@' }] },
};

describe('CollaborationPanorama', () => {
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

  it('renders named collaborators and reuses document and diff navigation', async () => {
    const onOpenDocument = vi.fn();
    const onOpenDiff = vi.fn();
    await act(async () => root.render(
      <CollaborationPanorama
        tasks={[task]}
        batchId="batch-1"
        rootTitle="统一异步委派"
        onOpenDocument={onOpenDocument}
        onOpenDiff={onOpenDiff}
      />,
    ));

    expect(container.textContent).toContain('主 Agent');
    expect(container.textContent).toContain('工程师');
    expect(container.textContent).toContain('Mia');
    expect(container.textContent).toContain('Codex');
    expect(container.textContent).toContain('已完成');

    await act(async () => {
      (container.querySelector('[title="阅读：任务闭环实现报告"]') as HTMLButtonElement).click();
    });
    expect(onOpenDocument).toHaveBeenCalledWith('doc-1', task, '任务闭环实现报告');

    await act(async () => {
      (container.querySelector('[title="审阅 1 个改动文件"]') as HTMLButtonElement).click();
    });
    expect(onOpenDiff).toHaveBeenCalledWith(expect.objectContaining({
      taskId: 'task-1',
      path: 'src/App.tsx',
      kind: 'diff',
    }), task);
  });

  it('exposes zoom controls without hiding the graph behind color-only status', async () => {
    await act(async () => root.render(
      <CollaborationPanorama
        tasks={[{ ...task, status: 'running', outputStatus: 'pending' }]}
        batchId="batch-1"
        onOpenDocument={vi.fn()}
        onOpenDiff={vi.fn()}
      />,
    ));
    expect(container.querySelector('[aria-label="放大全景图"]')).not.toBeNull();
    expect(container.querySelector('[aria-label="缩小全景图"]')).not.toBeNull();
    expect(container.querySelector('[aria-label="适应全景图"]')).not.toBeNull();
    expect(container.textContent).toContain('工作中');
  });
});
