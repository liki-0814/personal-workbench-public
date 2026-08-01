import { describe, expect, it } from 'vitest';
import type { RuntimeAttention, RuntimeTask } from '../state/taskRuntimeStore';
import { selectColdStartWorkbenchView, selectWorkItemProjection } from './workItemProjection';

function task(overrides: Partial<RuntimeTask>): RuntimeTask {
  return {
    id: 'task-1',
    batchId: 'batch-1',
    rootSessionId: 'runtime-session-1',
    kind: 'delegated',
    objective: '完成任务',
    cwd: '/workspace',
    status: 'running',
    deliveryStatus: 'pending',
    attempt: 1,
    createdAt: '2026-07-30T01:00:00.000Z',
    updatedAt: '2026-07-30T01:01:00.000Z',
    ...overrides,
  };
}

function attention(overrides: Partial<RuntimeAttention>): RuntimeAttention {
  return {
    id: 'attention-1',
    taskId: 'task-1',
    sessionId: 'runtime-session-1',
    kind: 'document_ready',
    status: 'unread',
    revision: 1,
    dedupeKey: 'task-1:ready',
    title: '产出待处理',
    createdAt: '2026-07-30T01:01:00.000Z',
    updatedAt: '2026-07-30T01:01:00.000Z',
    ...overrides,
  };
}

describe('work item projection', () => {
  it('classifies attention, background work, and resumable deliverables with session targets', () => {
    const projection = selectWorkItemProjection([
      task({ id: 'review', batchId: 'batch-review', status: 'succeeded', outputStatus: 'ready', reviewStatus: 'pending' }),
      task({ id: 'paused-callback', batchId: 'batch-callback', status: 'succeeded', outputStatus: 'ready', deliveryStatus: 'waiting_user' }),
      task({ id: 'running', batchId: 'batch-running', status: 'materializing', outputStatus: 'materializing' }),
      task({ id: 'document', batchId: 'batch-document', status: 'succeeded', outputStatus: 'ready', primaryDocumentId: 'doc-1' }),
      task({ id: 'cancelled', batchId: 'batch-cancelled', status: 'cancelled' }),
      task({ id: 'legacy-dispatch', batchId: 'batch-dispatch', kind: 'supervisor_dispatch', status: 'failed' }),
    ], [{ id: 'chat-1', title: '闭环实现', agentSessionId: 'runtime-session-1' }]);

    expect(projection.attention.map(item => item.task.id)).toEqual(['review', 'paused-callback']);
    expect(projection.running.map(item => item.task.id)).toEqual(['running']);
    expect(projection.continueWork.map(item => item.task.id)).toEqual(['document']);
    expect(projection.continueWork[0]).toMatchObject({ sessionId: 'chat-1', sessionTitle: '闭环实现' });
    expect(projection.badges).toEqual({ attention: 2, running: 1, continueWork: 1, total: 4 });
  });

  it('aggregates collaborators by work item and keeps migrated legacy results visible', () => {
    const projection = selectWorkItemProjection([
      task({ id: 'research', batchId: 'batch-collab', workItemId: 'todo-1', status: 'succeeded', outputStatus: 'ready', primaryDocumentId: 'doc-1' }),
      task({ id: 'review', batchId: 'batch-collab', workItemId: 'todo-1', status: 'succeeded', outputStatus: 'ready', reviewStatus: 'pending' }),
      task({ id: 'legacy-child', batchId: 'legacy-batch', kind: 'supervisor_child', status: 'succeeded', outputStatus: 'ready', primaryDocumentId: 'legacy-doc' }),
      task({ id: 'legacy-dispatch', batchId: 'transport-batch', kind: 'supervisor_dispatch', status: 'failed' }),
    ], []);

    expect(projection.attention).toHaveLength(1);
    expect(projection.attention[0]).toMatchObject({
      id: 'todo-1',
      task: { id: 'review' },
    });
    expect(projection.attention[0].tasks.map(item => item.id)).toEqual(['research', 'review']);
    expect(projection.continueWork.map(item => item.task.id)).toEqual(['legacy-child']);
    expect(projection.badges.total).toBe(2);
  });

  it('selects the inbox on cold start only for unresolved attention and never after a manual choice', () => {
    const attention = selectWorkItemProjection([
      task({ status: 'waiting_user' }),
    ], []);
    const running = selectWorkItemProjection([
      task({ status: 'running' }),
    ], []);

    expect(selectColdStartWorkbenchView(attention, 'day_plan', false)).toBe('inbox');
    expect(selectColdStartWorkbenchView(running, 'day_plan', false)).toBe('day_plan');
    expect(selectColdStartWorkbenchView(attention, 'habits', true)).toBe('habits');
  });

  it('keeps a read-only deliverable in attention until the user confirms it', () => {
    const pending = selectWorkItemProjection([
      task({
        access: 'read_only',
        status: 'succeeded',
        outputStatus: 'ready',
        reviewStatus: 'not_required',
        primaryDocumentId: 'doc-1',
      }),
    ], []);
    const confirmed = selectWorkItemProjection([
      task({
        access: 'read_only',
        status: 'succeeded',
        outputStatus: 'ready',
        reviewStatus: 'applied',
        primaryDocumentId: 'doc-1',
      }),
    ], []);

    expect(pending.attention).toHaveLength(1);
    expect(confirmed.continueWork).toHaveLength(1);
  });

  it('groups durable attention by work item and counts unread events instead of rows', () => {
    const projection = selectWorkItemProjection([
      task({ id: 'research', workItemId: 'todo-1', status: 'succeeded', outputStatus: 'ready' }),
      task({ id: 'review', workItemId: 'todo-1', status: 'succeeded', outputStatus: 'ready' }),
    ], [], [
      attention({ id: 'a-1', taskId: 'research', workItemId: 'todo-1' }),
      attention({ id: 'a-2', taskId: 'review', workItemId: 'todo-1', status: 'viewed' }),
    ]);

    expect(projection.attention).toHaveLength(1);
    expect(projection.attention[0]).toMatchObject({ id: 'todo-1', unreadCount: 1 });
    expect(projection.attention[0].attention.map(item => item.id)).toEqual(['a-1', 'a-2']);
    expect(projection.badges.attention).toBe(1);
    expect(projection.hasUnresolvedAttention).toBe(true);
  });

  it('does not recreate resolved durable attention from a terminal task status', () => {
    const completed = task({
      access: 'read_only',
      status: 'succeeded',
      outputStatus: 'ready',
      reviewStatus: 'not_required',
      primaryDocumentId: 'result-doc',
    });
    const pending = selectWorkItemProjection([completed], [], [
      attention({ taskId: completed.id, kind: 'document_ready' }),
    ]);
    const resolved = selectWorkItemProjection([completed], [], [
      attention({ taskId: completed.id, kind: 'document_ready', status: 'resolved' }),
    ]);

    expect(pending.attention).toHaveLength(1);
    expect(resolved.attention).toHaveLength(0);
    expect(resolved.continueWork).toHaveLength(0);
    expect(resolved.badges.total).toBe(0);
  });

  it('does not classify a failed task with pending output as background work', () => {
    const projection = selectWorkItemProjection([
      task({ status: 'failed', outputStatus: 'pending' }),
    ], [], []);

    expect(projection.attention).toHaveLength(0);
    expect(projection.running).toHaveLength(0);
    expect(projection.continueWork).toHaveLength(0);
  });

  it('keeps genuinely running work visible even when its old attention is resolved', () => {
    const running = task({ status: 'running', outputStatus: 'pending' });
    const projection = selectWorkItemProjection([running], [], [
      attention({ taskId: running.id, status: 'resolved' }),
    ]);

    expect(projection.running.map(item => item.task.id)).toEqual([running.id]);
    expect(projection.badges.total).toBe(1);
  });
});
