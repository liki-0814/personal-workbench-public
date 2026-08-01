import { describe, expect, it } from 'vitest';
import type { RuntimeTask } from '../../state/taskRuntimeStore';
import { layoutCollaboration, selectCollaborationLineage } from './collaborationLayout';

function task(id: string, patch: Partial<RuntimeTask> = {}): RuntimeTask {
  return {
    id,
    batchId: 'batch-main',
    rootSessionId: 'session-1',
    kind: 'delegated',
    objective: id,
    cwd: '/workspace',
    status: 'succeeded',
    deliveryStatus: 'delivered',
    attempt: 1,
    createdAt: `2026-07-30T01:00:${String(Number(id.replace(/\D/g, '')) || 0).padStart(2, '0')}.000Z`,
    updatedAt: '2026-07-30T02:00:00.000Z',
    ...patch,
  };
}

describe('collaborationLayout', () => {
  it('keeps the selected real batch lineage and its descendant batches', () => {
    const tasks = [
      task('root'),
      task('child-1', { parentTaskId: 'root' }),
      task('grandchild-2', { batchId: 'batch-child', parentTaskId: 'child-1' }),
      task('sibling-3', { batchId: 'batch-child', parentTaskId: 'root' }),
      task('unrelated-4', { batchId: 'batch-unrelated' }),
    ];

    const lineage = selectCollaborationLineage(tasks, 'batch-main');
    expect(lineage.map(item => item.id)).toEqual(['root', 'child-1', 'grandchild-2', 'sibling-3']);

    const layout = layoutCollaboration(lineage);
    expect(layout.edges.map(edge => `${edge.parentId}>${edge.childId}`)).toEqual(expect.arrayContaining([
      '__root__>root',
      'root>child-1',
      'child-1>grandchild-2',
      'root>sibling-3',
    ]));
    expect(layout.nodes.find(node => node.id === 'grandchild-2')?.depth).toBe(2);
  });

  it('lays out a large batch at stable finite coordinates without recursive indentation', () => {
    const tasks = Array.from({ length: 48 }, (_, index) => task(`task-${index + 1}`));
    const first = layoutCollaboration(tasks);
    const second = layoutCollaboration([...tasks].reverse());

    expect(first.nodes).toHaveLength(49);
    expect(first.width).toBeGreaterThan(920);
    expect(first.nodes.map(node => [node.id, node.x, node.y])).toEqual(
      second.nodes.map(node => [node.id, node.x, node.y]),
    );
    expect(first.nodes.every(node => Number.isFinite(node.x) && Number.isFinite(node.y))).toBe(true);
  });

  it('breaks malformed parent cycles into roots', () => {
    const layout = layoutCollaboration([
      task('task-1', { parentTaskId: 'task-2' }),
      task('task-2', { parentTaskId: 'task-1' }),
    ]);
    expect(layout.edges.map(edge => edge.parentId)).toEqual(['__root__', '__root__']);
  });
});
