import { describe, expect, it } from 'vitest';
import type { RuntimeTask } from './taskRuntimeStore';
import { selectRuntimeTasks } from './taskRuntimeStore';

function task(id: string, rootSessionId: string, kind = 'delegated'): RuntimeTask {
  return {
    id,
    batchId: `batch-${id}`,
    rootSessionId,
    kind,
    objective: id,
    cwd: '/workspace',
    status: 'running',
    deliveryStatus: 'pending',
    attempt: 1,
    createdAt: '2026-07-30T01:00:00.000Z',
    updatedAt: '2026-07-30T01:00:00.000Z',
  };
}

describe('runtime task selection', () => {
  it('keeps migrated legacy tasks visible while hiding transport-only dispatch rows', () => {
    const tasks = [
      task('one', 'session-1'),
      task('two', 'session-2'),
      task('legacy', 'session-1', 'supervisor_child'),
      task('dispatch', 'session-1', 'supervisor_dispatch'),
    ];

    expect(selectRuntimeTasks(tasks).map(item => item.id)).toEqual(['one', 'two', 'legacy']);
    expect(selectRuntimeTasks(tasks, 'session-2').map(item => item.id)).toEqual(['two']);
  });
});
