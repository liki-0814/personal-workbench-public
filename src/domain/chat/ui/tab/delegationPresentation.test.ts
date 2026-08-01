import { describe, expect, it } from 'vitest';
import type { RuntimeTask } from '../../state/taskRuntimeStore';
import {
  groupRuntimeTaskBatches,
  isTaskActive,
  presentRuntimeTask,
  runtimeTaskDepth,
} from './delegationPresentation';

function task(overrides: Partial<RuntimeTask> = {}): RuntimeTask {
  return {
    id: 'task-1',
    batchId: 'batch-1',
    rootSessionId: 'session-1',
    kind: 'delegated',
    objective: '完成设计审阅',
    cwd: '/workspace',
    status: 'running',
    deliveryStatus: 'pending',
    attempt: 1,
    createdAt: '2026-07-30T01:00:00.000Z',
    updatedAt: '2026-07-30T01:01:00.000Z',
    ...overrides,
  };
}

describe('delegation presentation', () => {
  it('uses persisted identity and resolved executor while keeping legacy result compatibility', () => {
    const value = presentRuntimeTask(task({
      role: 'researcher',
      roleLabel: '调研员',
      displayName: 'Alex',
      avatarSeed: 'stable-alex',
      executorRequest: 'auto',
      resolvedExecutorId: 'codex',
      resolvedModel: 'gpt-5.6',
      routingReason: '调研角色首选 Codex',
      status: 'succeeded',
      outputStatus: 'ready',
      result: {
        document: { id: 'doc-1', title: '简洁性视角设计方案', format: 'markdown' },
        changedFiles: [{ path: 'src/App.tsx', patch: '@@ -1 +1 @@' }],
        tokenUsage: { totalTokens: 1234 },
      },
    }));

    expect(value).toMatchObject({
      roleLabel: '调研员',
      displayName: 'Alex',
      executorLabel: 'Codex',
      statusLabel: '已完成',
      documentId: 'doc-1',
      documentTitle: '简洁性视角设计方案',
      modelLabel: 'gpt-5.6',
      routingModeLabel: '自动选择',
      totalTokens: 1234,
    });
    expect(value.changedFiles).toHaveLength(1);
    expect(presentRuntimeTask(task({ avatarSeed: 'stable-alex' })).avatarTone).toBe(value.avatarTone);
  });

  it('does not present a succeeded task as complete until its document is ready', () => {
    const value = task({ status: 'succeeded', outputStatus: 'materializing' });
    expect(presentRuntimeTask(value).statusLabel).toBe('正在整理产出');
    expect(isTaskActive(value)).toBe(true);
  });

  it('keeps a mutating task visible while its diff needs review', () => {
    const value = task({
      status: 'succeeded',
      outputStatus: 'ready',
      reviewStatus: 'pending',
    });
    expect(presentRuntimeTask(value).statusLabel).toBe('待你审阅');
    expect(isTaskActive(value)).toBe(true);
  });

  it('keeps grandchildren out of the primary batch rows', () => {
    const root = task({ id: 'root', depth: undefined });
    const child = task({ id: 'child', parentTaskId: 'root', depth: undefined });
    const grandchild = task({ id: 'grandchild', parentTaskId: 'child', depth: undefined });
    const tasks = [root, child, grandchild];
    const [batch] = groupRuntimeTaskBatches(tasks);

    expect(runtimeTaskDepth(grandchild, tasks)).toBe(2);
    expect(batch.visibleTasks.map(item => item.id)).toEqual(['root', 'child']);
    expect(batch.deepTasks.map(item => item.id)).toEqual(['grandchild']);
  });
});
