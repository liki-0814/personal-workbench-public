import type { ChatSession } from '../types';
import type { RuntimeAttention, RuntimeTask } from '../state/taskRuntimeStore';

export type WorkItemSection = 'attention' | 'running' | 'continue';
export type WorkItemNavigationTarget =
  | { kind: 'card' }
  | { kind: 'document'; documentId: string }
  | { kind: 'diff'; path: string };

export interface RuntimeNavigationRequest {
  id: number;
  taskId: string;
  sessionId?: string;
  target: WorkItemNavigationTarget;
}

export interface WorkItem {
  /** Stable logical work-item key. A batch of collaborators is one inbox row. */
  id: string;
  task: RuntimeTask;
  tasks: RuntimeTask[];
  attention: RuntimeAttention[];
  unreadCount: number;
  sessionId?: string;
  sessionTitle?: string;
}

export interface WorkItemProjection {
  attention: WorkItem[];
  running: WorkItem[];
  continueWork: WorkItem[];
  badges: {
    attention: number;
    running: number;
    continueWork: number;
    total: number;
  };
  hasUnresolvedAttention: boolean;
}

const TERMINAL_STATUSES = new Set(['succeeded', 'failed', 'cancelled', 'recovery_required']);
const ATTENTION_STATUSES = new Set(['waiting_user', 'waiting_configuration', 'failed', 'recovery_required']);
const SECTION_PRIORITY: Record<WorkItemSection, number> = {
  attention: 3,
  running: 2,
  continue: 1,
};

function resultObject(task: RuntimeTask): Record<string, unknown> | undefined {
  return task.result != null && typeof task.result === 'object' && !Array.isArray(task.result)
    ? task.result as Record<string, unknown>
    : undefined;
}

function hasDeliverable(task: RuntimeTask): boolean {
  const result = resultObject(task);
  return Boolean(
    task.primaryDocumentId
    || result?.primaryDocumentId
    || result?.document
    || (Array.isArray(result?.changedFiles) && result.changedFiles.length > 0),
  );
}

export function workItemSection(task: RuntimeTask): WorkItemSection | null {
  if (
    ATTENTION_STATUSES.has(task.status)
    || task.deliveryStatus === 'waiting_user'
    || task.outputStatus === 'failed'
    || task.reviewStatus === 'pending'
    || task.reviewStatus === 'merge_required'
    || (
      task.access === 'read_only'
      && task.status === 'succeeded'
      && task.outputStatus === 'ready'
      && task.reviewStatus !== 'applied'
      && task.reviewStatus !== 'rejected'
    )
  ) return 'attention';

  return workItemSectionWithoutAttention(task);
}

function workItemSectionWithoutAttention(task: RuntimeTask): WorkItemSection | null {
  if (!TERMINAL_STATUSES.has(task.status)) return 'running';

  if (task.status === 'failed' || task.status === 'recovery_required' || task.status === 'cancelled') {
    return null;
  }

  if (
    task.outputStatus === 'pending'
    || task.outputStatus === 'materializing'
    || task.reviewStatus === 'changes_requested'
  ) return 'running';

  if (hasDeliverable(task)) return 'continue';
  return null;
}

export function selectWorkItemProjection(
  tasks: RuntimeTask[],
  sessions: Pick<ChatSession, 'id' | 'title' | 'agentSessionId'>[],
  attention?: RuntimeAttention[],
): WorkItemProjection {
  const sessionByRuntimeId = new Map<string, Pick<ChatSession, 'id' | 'title' | 'agentSessionId'>>();
  sessions.forEach(session => {
    sessionByRuntimeId.set(session.id, session);
    if (session.agentSessionId) sessionByRuntimeId.set(session.agentSessionId, session);
  });

  const sections: Record<WorkItemSection, WorkItem[]> = {
    attention: [],
    running: [],
    continue: [],
  };
  const grouped = new Map<string, RuntimeTask[]>();
  const taskById = new Map(tasks.map(task => [task.id, task]));
  const attentionByGroup = new Map<string, RuntimeAttention[]>();
  attention?.forEach(item => {
    const task = taskById.get(item.taskId);
    const key = item.workItemId || task?.workItemId || task?.batchId || item.taskId;
    const group = attentionByGroup.get(key) ?? [];
    group.push(item);
    attentionByGroup.set(key, group);
  });
  [...tasks]
    // A migrated root/child remains readable as a legacy collaborator. Only the
    // old transport-level dispatch row is an implementation detail.
    .filter(task => task.kind !== 'supervisor_dispatch')
    .forEach(task => {
      const key = task.workItemId || task.batchId || task.id;
      const group = grouped.get(key) ?? [];
      group.push(task);
      grouped.set(key, group);
    });

  [...grouped.entries()]
    .map<{ section: WorkItemSection; item: WorkItem } | null>(([id, group]) => {
      const durableAttention = [...(attentionByGroup.get(id) ?? [])]
        .sort((left, right) => (
          Number(right.status === 'unread') - Number(left.status === 'unread')
          || Date.parse(right.updatedAt) - Date.parse(left.updatedAt)
        ));
      const activeAttention = durableAttention.filter(item => item.status !== 'resolved');
      const hasResolvedTombstone = durableAttention.some(item => item.status === 'resolved');
      const visible = group
        .map(task => {
          const inferredSection = workItemSection(task);
          const section = activeAttention.some(item => item.taskId === task.id)
            ? 'attention' as const
            : attention !== undefined && inferredSection === 'attention'
              ? workItemSectionWithoutAttention(task)
              : inferredSection;
          return {
            task,
            section: activeAttention.length === 0
              && hasResolvedTombstone
              && TERMINAL_STATUSES.has(task.status)
              ? null
              : section,
          };
        })
        .filter((entry): entry is { task: RuntimeTask; section: WorkItemSection } => entry.section != null)
        .sort((left, right) => (
          SECTION_PRIORITY[right.section] - SECTION_PRIORITY[left.section]
          || Date.parse(right.task.updatedAt) - Date.parse(left.task.updatedAt)
        ));
      if (visible.length === 0) return null;
      let representative = visible[0];
      if (activeAttention.length > 0) {
        const attentionTask = visible.find(entry => entry.task.id === activeAttention[0].taskId);
        if (attentionTask) representative = { ...attentionTask, section: 'attention' };
      }
      const orderedTasks = [...group].sort((left, right) => Date.parse(right.updatedAt) - Date.parse(left.updatedAt));
      const session = sessionByRuntimeId.get(representative.task.rootSessionId);
      const item: WorkItem = {
        id,
        task: representative.task,
        tasks: orderedTasks,
        attention: activeAttention,
        unreadCount: activeAttention.filter(item => item.status === 'unread').length,
        sessionId: session?.id,
        sessionTitle: session?.title,
      };
      return {
        section: representative.section,
        item,
      };
    })
    .filter((entry): entry is { section: WorkItemSection; item: WorkItem } => entry !== null)
    .sort((left, right) => Date.parse(right.item.task.updatedAt) - Date.parse(left.item.task.updatedAt))
    .forEach(({ section, item }) => sections[section].push(item));

  const badges = {
    attention: attention === undefined
      ? sections.attention.length
      : sections.attention.reduce((sum, item) => sum + item.unreadCount, 0),
    running: sections.running.length,
    continueWork: sections.continue.length,
    total: sections.attention.length + sections.running.length + sections.continue.length,
  };
  return {
    attention: sections.attention,
    running: sections.running,
    continueWork: sections.continue,
    badges,
    hasUnresolvedAttention: attention === undefined
      ? badges.attention > 0
      : sections.attention.some(item => item.attention.some(entry => entry.status !== 'resolved')),
  };
}

export function selectColdStartWorkbenchView<T extends string>(
  projection: WorkItemProjection,
  fallback: T,
  hasManualSelection: boolean,
): T | 'inbox' {
  return !hasManualSelection && projection.hasUnresolvedAttention ? 'inbox' : fallback;
}
