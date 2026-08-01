import type { RuntimeTask } from '../../state/taskRuntimeStore';

export const COLLABORATION_NODE_WIDTH = 236;
export const COLLABORATION_NODE_HEIGHT = 112;

const HORIZONTAL_GAP = 36;
const VERTICAL_GAP = 70;
const CANVAS_PADDING = 56;
const MIN_CANVAS_WIDTH = 920;

export interface CollaborationLayoutNode {
  id: string;
  task?: RuntimeTask;
  parentId?: string;
  depth: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface CollaborationLayoutEdge {
  id: string;
  parentId: string;
  childId: string;
  path: string;
}

export interface CollaborationLayout {
  nodes: CollaborationLayoutNode[];
  edges: CollaborationLayoutEdge[];
  width: number;
  height: number;
}

function stableTaskOrder(left: RuntimeTask, right: RuntimeTask): number {
  const timeDelta = Date.parse(left.createdAt) - Date.parse(right.createdAt);
  return Number.isFinite(timeDelta) && timeDelta !== 0 ? timeDelta : left.id.localeCompare(right.id);
}

/** Selects one connected collaboration lineage while keeping descendant batches. */
export function selectCollaborationLineage(tasks: RuntimeTask[], batchId?: string): RuntimeTask[] {
  if (!batchId) return [...tasks].sort(stableTaskOrder);
  const selected = new Set(tasks.filter(task => task.batchId === batchId).map(task => task.id));
  if (selected.size === 0) return [];

  let changed = true;
  while (changed) {
    changed = false;
    for (const task of tasks) {
      if (selected.has(task.id) && task.parentTaskId && !selected.has(task.parentTaskId)) {
        selected.add(task.parentTaskId);
        changed = true;
      }
      if (task.parentTaskId && selected.has(task.parentTaskId) && !selected.has(task.id)) {
        selected.add(task.id);
        changed = true;
      }
    }
  }
  return tasks.filter(task => selected.has(task.id)).sort(stableTaskOrder);
}

function safeParentIds(tasks: RuntimeTask[]): Map<string, string> {
  const byId = new Map(tasks.map(task => [task.id, task]));
  const parents = new Map<string, string>();
  for (const task of tasks) {
    const parentId = task.parentTaskId;
    if (!parentId || parentId === task.id || !byId.has(parentId)) continue;
    const seen = new Set([task.id]);
    let cursor: RuntimeTask | undefined = byId.get(parentId);
    let cyclic = false;
    while (cursor) {
      if (seen.has(cursor.id)) {
        cyclic = true;
        break;
      }
      seen.add(cursor.id);
      cursor = cursor.parentTaskId ? byId.get(cursor.parentTaskId) : undefined;
    }
    if (!cyclic) parents.set(task.id, parentId);
  }
  return parents;
}

function edgePath(parent: CollaborationLayoutNode, child: CollaborationLayoutNode): string {
  const startX = parent.x + parent.width / 2;
  const startY = parent.y + parent.height;
  const endX = child.x + child.width / 2;
  const endY = child.y;
  const middleY = startY + (endY - startY) * 0.5;
  return `M ${startX} ${startY} C ${startX} ${middleY}, ${endX} ${middleY}, ${endX} ${endY}`;
}

export function layoutCollaboration(tasks: RuntimeTask[]): CollaborationLayout {
  const sorted = [...tasks].sort(stableTaskOrder);
  const parentIds = safeParentIds(sorted);
  const children = new Map<string, RuntimeTask[]>();
  const roots: RuntimeTask[] = [];
  for (const task of sorted) {
    const parentId = parentIds.get(task.id);
    if (!parentId) {
      roots.push(task);
      continue;
    }
    const siblings = children.get(parentId);
    if (siblings) siblings.push(task);
    else children.set(parentId, [task]);
  }
  children.forEach(items => items.sort(stableTaskOrder));

  const placed = new Map<string, CollaborationLayoutNode>();
  let leafCursor = CANVAS_PADDING;
  let maximumDepth = 0;
  const place = (task: RuntimeTask, depth: number): CollaborationLayoutNode => {
    const taskChildren = children.get(task.id) ?? [];
    const childNodes = taskChildren.map(child => place(child, depth + 1));
    let x: number;
    if (childNodes.length === 0) {
      x = leafCursor;
      leafCursor += COLLABORATION_NODE_WIDTH + HORIZONTAL_GAP;
    } else {
      const firstCenter = childNodes[0].x + childNodes[0].width / 2;
      const lastCenter = childNodes[childNodes.length - 1].x + childNodes[childNodes.length - 1].width / 2;
      x = (firstCenter + lastCenter) / 2 - COLLABORATION_NODE_WIDTH / 2;
    }
    maximumDepth = Math.max(maximumDepth, depth);
    const node: CollaborationLayoutNode = {
      id: task.id,
      task,
      parentId: parentIds.get(task.id) ?? '__root__',
      depth,
      x,
      y: CANVAS_PADDING + (depth + 1) * (COLLABORATION_NODE_HEIGHT + VERTICAL_GAP),
      width: COLLABORATION_NODE_WIDTH,
      height: COLLABORATION_NODE_HEIGHT,
    };
    placed.set(task.id, node);
    return node;
  };
  const rootTaskNodes = roots.map(task => place(task, 0));

  const usedWidth = Math.max(
    COLLABORATION_NODE_WIDTH,
    ...Array.from(placed.values(), node => node.x + node.width - CANVAS_PADDING),
  );
  const width = Math.max(MIN_CANVAS_WIDTH, usedWidth + CANVAS_PADDING * 2);
  const horizontalOffset = Math.max(0, (width - usedWidth - CANVAS_PADDING * 2) / 2);
  placed.forEach(node => { node.x += horizontalOffset; });

  const firstRootCenter = rootTaskNodes[0]
    ? placed.get(rootTaskNodes[0].id)!.x + COLLABORATION_NODE_WIDTH / 2
    : width / 2;
  const lastRootCenter = rootTaskNodes.length > 0
    ? placed.get(rootTaskNodes[rootTaskNodes.length - 1].id)!.x + COLLABORATION_NODE_WIDTH / 2
    : width / 2;
  const rootNode: CollaborationLayoutNode = {
    id: '__root__',
    depth: -1,
    x: (firstRootCenter + lastRootCenter) / 2 - COLLABORATION_NODE_WIDTH / 2,
    y: CANVAS_PADDING,
    width: COLLABORATION_NODE_WIDTH,
    height: COLLABORATION_NODE_HEIGHT,
  };
  const nodes = [rootNode, ...sorted.map(task => placed.get(task.id)!).filter(Boolean)];
  const byNodeId = new Map(nodes.map(node => [node.id, node]));
  const edges = nodes.flatMap(node => {
    if (!node.task || !node.parentId) return [];
    const parent = byNodeId.get(node.parentId);
    if (!parent) return [];
    return [{
      id: `${parent.id}:${node.id}`,
      parentId: parent.id,
      childId: node.id,
      path: edgePath(parent, node),
    }];
  });
  return {
    nodes,
    edges,
    width,
    height: CANVAS_PADDING * 2 + (maximumDepth + 2) * (COLLABORATION_NODE_HEIGHT + VERTICAL_GAP),
  };
}
