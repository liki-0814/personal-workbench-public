import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import WorkbenchRightPanel from './WorkbenchRightPanel';
import type { RuntimeAttention, RuntimeTask, WorkItemProjection } from '@/domain/chat';

function runtimeTask(overrides: Partial<RuntimeTask>): RuntimeTask {
  return {
    id: 'task-review',
    batchId: 'batch-review',
    rootSessionId: 'session-review',
    kind: 'delegated',
    objective: '审阅协作者改动',
    cwd: '/workspace',
    status: 'succeeded',
    deliveryStatus: 'review_ready',
    attempt: 1,
    createdAt: '2026-07-30T01:00:00.000Z',
    updatedAt: '2026-07-30T01:01:00.000Z',
    outputStatus: 'ready',
    primaryDocumentId: 'doc-review',
    reviewStatus: 'pending',
    result: { changedFiles: [{ path: 'src/App.tsx', patch: '@@ -1 +1 @@' }] },
    ...overrides,
  };
}

describe('WorkbenchRightPanel', () => {
  let container: HTMLDivElement | null = null;

  afterEach(() => {
    container?.remove();
    container = null;
  });

  it('exposes tab semantics and keyboard navigation', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    const onActiveViewChange = vi.fn();
    await act(async () => root.render(
      <WorkbenchRightPanel
        activeView="day_plan"
        onActiveViewChange={onActiveViewChange}
        todos={[]}
        dayPlans={[]}
        objectivePanel={<div>任务</div>}
        dayPlanPanel={<div>计划</div>}
        projects={[]}
        onCaptureWorkItem={vi.fn()}
      />,
    ));

    const tabs = Array.from(container.querySelectorAll<HTMLButtonElement>('[role="tab"]'));
    expect(tabs).toHaveLength(5);
    expect(Array.from(container.querySelectorAll('button')).some(button => button.textContent?.includes('捕获事项'))).toBe(true);
    expect(tabs[2].getAttribute('aria-selected')).toBe('true');
    await act(async () => tabs[2].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })));
    expect(onActiveViewChange).toHaveBeenCalledWith('habits');
    expect(document.activeElement).toBe(tabs[3]);
    await act(async () => root.unmount());
  });

  it('opens review attention directly in the third-screen diff target', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    const onOpenWorkItem = vi.fn();
    const task = runtimeTask({});
    const projection: WorkItemProjection = {
      attention: [{
        id: task.workItemId || task.batchId,
        task,
        tasks: [task],
        attention: [],
        unreadCount: 0,
        sessionId: 'chat-review',
        sessionTitle: '审阅会话',
      }],
      running: [],
      continueWork: [],
      badges: { attention: 1, running: 0, continueWork: 0, total: 1 },
      hasUnresolvedAttention: true,
    };
    await act(async () => root.render(
      <WorkbenchRightPanel
        activeView="inbox"
        onActiveViewChange={vi.fn()}
        todos={[]}
        dayPlans={[]}
        objectivePanel={null}
        dayPlanPanel={null}
        workItems={projection}
        onOpenWorkItem={onOpenWorkItem}
        projects={[]}
        onCaptureWorkItem={vi.fn()}
      />,
    ));

    await act(async () => {
      (container?.querySelector('.work-inbox-item-main') as HTMLButtonElement).click();
    });
    expect(onOpenWorkItem).toHaveBeenCalledWith(
      expect.objectContaining({ task }),
      { kind: 'diff', path: 'src/App.tsx' },
    );
    await act(async () => root.unmount());
  });

  it('resolves durable attention with pending and failure feedback', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    let rejectRequest: ((reason?: unknown) => void) | undefined;
    const onResolveAttention = vi.fn(() => new Promise<void>((_resolve, reject) => {
      rejectRequest = reject;
    }));
    const task = runtimeTask({ status: 'failed', reviewStatus: 'not_required' });
    const attention: RuntimeAttention = {
      id: 'attention-failed',
      taskId: task.id,
      sessionId: task.rootSessionId,
      kind: 'task_failed',
      status: 'unread',
      revision: 1,
      dedupeKey: 'task-failed',
      title: '执行失败',
      createdAt: task.createdAt,
      updatedAt: task.updatedAt,
    };
    const projection: WorkItemProjection = {
      attention: [{
        id: task.batchId,
        task,
        tasks: [task],
        attention: [attention],
        unreadCount: 1,
        sessionId: 'chat-review',
      }],
      running: [],
      continueWork: [],
      badges: { attention: 1, running: 0, continueWork: 0, total: 1 },
      hasUnresolvedAttention: true,
    };
    await act(async () => root.render(
      <WorkbenchRightPanel
        activeView="inbox"
        onActiveViewChange={vi.fn()}
        todos={[]}
        dayPlans={[]}
        objectivePanel={null}
        dayPlanPanel={null}
        workItems={projection}
        onOpenWorkItem={vi.fn()}
        onResolveAttention={onResolveAttention}
        projects={[]}
        onCaptureWorkItem={vi.fn()}
      />,
    ));

    const resolve = container.querySelector<HTMLButtonElement>('[title="标为已处理"]')!;
    await act(async () => resolve.click());
    expect(onResolveAttention).toHaveBeenCalledWith('attention-failed');
    expect(resolve.disabled).toBe(true);
    expect(resolve.getAttribute('aria-label')).toContain('正在标记');

    await act(async () => rejectRequest?.(new Error('状态已变化，请刷新')));
    expect(resolve.disabled).toBe(false);
    expect(container.querySelector('[role="alert"]')?.textContent).toBe('状态已变化，请刷新');
    await act(async () => root.unmount());
  });

  it('deletes every durable attention in one inbox row with single-flight failure feedback', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    const pending = new Map<string, { resolve: () => void; reject: (reason?: unknown) => void }>();
    const onDeleteAttention = vi.fn((attentionId: string) => new Promise<void>((resolve, reject) => {
      pending.set(attentionId, { resolve, reject });
    }));
    const task = runtimeTask({ status: 'failed', reviewStatus: 'not_required' });
    const attention = (id: string): RuntimeAttention => ({
      id,
      taskId: task.id,
      sessionId: task.rootSessionId,
      kind: 'task_failed',
      status: 'unread',
      revision: 1,
      dedupeKey: id,
      title: '执行失败',
      createdAt: task.createdAt,
      updatedAt: task.updatedAt,
    });
    const projection: WorkItemProjection = {
      attention: [{
        id: task.batchId,
        task,
        tasks: [task],
        attention: [attention('attention-one'), attention('attention-two')],
        unreadCount: 2,
      }],
      running: [],
      continueWork: [],
      badges: { attention: 2, running: 0, continueWork: 0, total: 1 },
      hasUnresolvedAttention: true,
    };
    await act(async () => root.render(
      <WorkbenchRightPanel
        activeView="inbox"
        onActiveViewChange={vi.fn()}
        todos={[]}
        dayPlans={[]}
        objectivePanel={null}
        dayPlanPanel={null}
        workItems={projection}
        onDeleteAttention={onDeleteAttention}
        projects={[]}
        onCaptureWorkItem={vi.fn()}
      />,
    ));

    const deleteButton = container.querySelector<HTMLButtonElement>('[title^="删除收件箱记录"]')!;
    await act(async () => {
      deleteButton.click();
      deleteButton.click();
    });
    expect(onDeleteAttention).toHaveBeenCalledTimes(2);
    expect(onDeleteAttention).toHaveBeenCalledWith('attention-one');
    expect(onDeleteAttention).toHaveBeenCalledWith('attention-two');
    expect(deleteButton.disabled).toBe(true);
    expect(deleteButton.getAttribute('aria-label')).toContain('正在删除');

    await act(async () => {
      pending.get('attention-one')?.resolve();
      pending.get('attention-two')?.reject(new Error('数据库繁忙，请重试'));
    });
    expect(deleteButton.disabled).toBe(false);
    expect(container.querySelector('[role="alert"]')?.textContent).toBe('数据库繁忙，请重试');
    await act(async () => root.unmount());
  });
});
