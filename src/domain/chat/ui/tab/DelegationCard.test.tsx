import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RuntimeTask } from '../../state/taskRuntimeStore';
import DelegationCard from './DelegationCard';

const completedTask: RuntimeTask = {
  id: 'task-1',
  batchId: 'batch-1',
  rootSessionId: 'session-1',
  kind: 'delegated',
  objective: '完成交互方案',
  cwd: '/workspace',
  status: 'succeeded',
  deliveryStatus: 'delivered',
  attempt: 1,
  createdAt: '2026-07-30T01:00:00.000Z',
  updatedAt: '2026-07-30T01:01:00.000Z',
  role: 'engineer',
  access: 'mutating',
  roleLabel: '工程师',
  displayName: 'Mia',
  resolvedExecutorId: 'codex',
  executorRequest: 'auto',
  resolvedModel: 'gpt-5.6',
  routingReason: '工程任务首选 CLI',
  primaryDocumentId: 'doc-1',
  deliverableTitle: '任务闭环实现报告',
  outputStatus: 'ready',
  result: {
    changedFiles: [{ path: 'src/App.tsx', patch: '@@ -1 +1 @@' }],
  },
};

describe('DelegationCard', () => {
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

  it('opens the persisted document and diff from a named collaborator card', async () => {
    const onOpenDocument = vi.fn();
    const onOpenDiff = vi.fn();
    await act(async () => root.render(
      <DelegationCard
        tasks={[completedTask]}
        onCancel={vi.fn()}
        onFollowUp={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenDocument={onOpenDocument}
        onOpenDiff={onOpenDiff}
      />,
    ));
    await act(async () => {
      (container.querySelector('.delegation-card-summary') as HTMLButtonElement).click();
    });

    expect(container.textContent).toContain('工程师 Mia');
    expect(container.textContent).toContain('Codex');
    expect(container.textContent).toContain('已完成');
    expect(container.textContent).toContain('任务闭环实现报告');

    await act(async () => {
      (container.querySelector('.delegation-deliverable') as HTMLButtonElement).click();
    });
    expect(onOpenDocument).toHaveBeenCalledWith('doc-1', completedTask);

    await act(async () => {
      (container.querySelector('.delegation-file-links button') as HTMLButtonElement).click();
    });
    expect(onOpenDiff).toHaveBeenCalledWith(expect.objectContaining({
      path: 'src/App.tsx',
      taskId: 'task-1',
      kind: 'diff',
    }), completedTask);
  });

  it('opens the collaboration panorama for the real runtime batch', async () => {
    const onOpenPanorama = vi.fn();
    await act(async () => root.render(
      <DelegationCard
        tasks={[completedTask]}
        onCancel={vi.fn()}
        onFollowUp={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenPanorama={onOpenPanorama}
      />,
    ));

    const panorama = Array.from(container.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.textContent?.includes('协作全景图'));
    await act(async () => panorama?.click());

    expect(onOpenPanorama).toHaveBeenCalledWith('batch-1');
  });

  it('collects follow-up requirements inline without using a browser prompt', async () => {
    const onFollowUp = vi.fn().mockResolvedValue(undefined);
    const prompt = vi.spyOn(window, 'prompt');
    await act(async () => root.render(
      <DelegationCard
        tasks={[completedTask]}
        onCancel={vi.fn()}
        onFollowUp={onFollowUp}
        onOpenFile={vi.fn()}
      />,
    ));
    await act(async () => {
      (container.querySelector('.delegation-card-summary') as HTMLButtonElement).click();
    });
    await act(async () => {
      (container.querySelector('[title="追加要求"]') as HTMLButtonElement).click();
    });

    const input = container.querySelector('.delegation-follow-up input') as HTMLInputElement;
    await act(async () => {
      const valueSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
      valueSetter?.call(input, '补充键盘交互说明');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await act(async () => {
      (container.querySelector('.delegation-follow-up') as HTMLFormElement)
        .dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    });

    expect(prompt).not.toHaveBeenCalled();
    expect(onFollowUp).toHaveBeenCalledWith('task-1', '补充键盘交互说明');
  });

  it('sends review actions with the current revision from the inline card', async () => {
    const onReview = vi.fn().mockResolvedValue(undefined);
    await act(async () => root.render(
      <DelegationCard
        tasks={[{ ...completedTask, reviewStatus: 'pending', reviewRevision: 3 }]}
        onCancel={vi.fn()}
        onFollowUp={vi.fn()}
        onReview={onReview}
        onOpenFile={vi.fn()}
      />,
    ));
    await act(async () => {
      (container.querySelector('.delegation-card-summary') as HTMLButtonElement).click();
    });
    const apply = Array.from(container.querySelectorAll<HTMLButtonElement>('.delegation-review-actions button'))
      .find(button => button.textContent === '应用改动');
    await act(async () => apply?.click());

    const confirm = Array.from(container.querySelectorAll<HTMLButtonElement>('.delegation-apply-confirm-actions button'))
      .find(button => button.textContent === '应用并确认');
    await act(async () => confirm?.click());

    expect(onReview).toHaveBeenCalledWith('task-1', 'apply', 3, undefined, {
      taskUpdate: undefined,
      memoryEntries: [],
    });
  });

  it('offers retry when an explicit executor needs configuration', async () => {
    const onRetry = vi.fn().mockResolvedValue(undefined);
    await act(async () => root.render(
      <DelegationCard
        tasks={[{ ...completedTask, status: 'waiting_configuration', outputStatus: 'pending' }]}
        onCancel={vi.fn()}
        onRetry={onRetry}
        onFollowUp={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    ));
    await act(async () => {
      (container.querySelector('.delegation-card-summary') as HTMLButtonElement).click();
    });
    await act(async () => {
      (container.querySelector('[title="配置完成后重试"]') as HTMLButtonElement).click();
    });
    expect(onRetry).toHaveBeenCalledWith('task-1');
  });

  it('confirms a read-only deliverable before projecting Todo and Memory updates', async () => {
    const onReview = vi.fn().mockResolvedValue(undefined);
    await act(async () => root.render(
      <DelegationCard
        tasks={[{
          ...completedTask,
          access: 'read_only',
          reviewStatus: 'not_required',
          workItemId: 'todo-1',
          result: { decisionCandidates: [{ id: 'decision-1', content: '统一使用 RuntimeTask' }] },
        }]}
        onCancel={vi.fn()}
        onFollowUp={vi.fn()}
        onReview={onReview}
        onOpenFile={vi.fn()}
      />,
    ));
    await act(async () => {
      (container.querySelector('.delegation-card-summary') as HTMLButtonElement).click();
    });
    const start = Array.from(container.querySelectorAll<HTMLButtonElement>('.delegation-review-actions button'))
      .find(button => button.textContent === '确认完成');
    await act(async () => start?.click());
    const confirm = Array.from(container.querySelectorAll<HTMLButtonElement>('.delegation-apply-confirm-actions button'))
      .find(button => button.textContent === '确认完成');
    await act(async () => confirm?.click());

    expect(onReview).toHaveBeenCalledWith('task-1', 'confirm', 0, undefined, {
      taskUpdate: { todoId: 'todo-1', markComplete: false },
      memoryEntries: [],
    });
  });

  it('shows a durable ACP decision and resumes with the selected option', async () => {
    const onResolveDecision = vi.fn().mockResolvedValue(undefined);
    await act(async () => root.render(
      <DelegationCard
        tasks={[{
          ...completedTask,
          status: 'waiting_user',
          outputStatus: 'pending',
          result: {
            question: '应该保留兼容接口吗？',
            options: ['保留一版', '立即删除'],
            nativeSessionId: 'codex-session-1',
          },
        }]}
        onCancel={vi.fn()}
        onResolveDecision={onResolveDecision}
        onFollowUp={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    ));

    expect(container.textContent).toContain('应该保留兼容接口吗？');
    const option = Array.from(container.querySelectorAll<HTMLButtonElement>('.delegation-decision-request button'))
      .find(button => button.textContent === '保留一版');
    await act(async () => option?.click());
    expect(onResolveDecision).toHaveBeenCalledWith('task-1', '保留一版');
  });

  it('only switches executor after an explicit inline selection', async () => {
    const onRetry = vi.fn().mockResolvedValue(undefined);
    const prompt = vi.spyOn(window, 'prompt');
    await act(async () => root.render(
      <DelegationCard
        tasks={[{ ...completedTask, status: 'failed', outputStatus: 'ready' }]}
        onCancel={vi.fn()}
        onRetry={onRetry}
        onFollowUp={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    ));
    await act(async () => {
      (container.querySelector('.delegation-card-summary') as HTMLButtonElement).click();
    });
    await act(async () => {
      (container.querySelector('[title="选择其他执行器重试"]') as HTMLButtonElement).click();
    });
    expect(onRetry).not.toHaveBeenCalled();
    await act(async () => {
      (container.querySelector('[aria-label="选择重试执行器"]') as HTMLButtonElement).click();
    });
    const kimi = Array.from(document.body.querySelectorAll<HTMLButtonElement>('[role="option"]'))
      .find(button => button.textContent?.includes('Kimi'));
    await act(async () => kimi?.click());
    const confirm = Array.from(container.querySelectorAll<HTMLButtonElement>('.delegation-retry-picker > button'))
      .find(button => button.textContent === '重试');
    await act(async () => confirm?.click());

    expect(prompt).not.toHaveBeenCalled();
    expect(onRetry).toHaveBeenCalledWith('task-1', 'kimi');
  });

  it('disables cancellation while pending and surfaces cancellation errors', async () => {
    let rejectCancel: ((reason?: unknown) => void) | undefined;
    const onCancel = vi.fn(() => new Promise<void>((_resolve, reject) => {
      rejectCancel = reject;
    }));
    await act(async () => root.render(
      <DelegationCard
        tasks={[{ ...completedTask, status: 'running', outputStatus: 'pending' }]}
        onCancel={onCancel}
        onFollowUp={vi.fn()}
        onOpenFile={vi.fn()}
      />,
    ));

    const cancel = container.querySelector<HTMLButtonElement>('[title="取消任务"]')!;
    await act(async () => cancel.click());
    expect(cancel.disabled).toBe(true);
    expect(cancel.getAttribute('aria-label')).toBe('正在取消任务');
    await act(async () => cancel.click());
    expect(onCancel).toHaveBeenCalledTimes(1);

    await act(async () => rejectCancel?.(new Error('取消请求失败')));
    expect(cancel.disabled).toBe(false);
    expect(container.querySelector('[role="alert"]')?.textContent).toBe('取消请求失败');
  });
});
