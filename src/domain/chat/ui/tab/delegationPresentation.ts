import type {
  RuntimeTask,
  RuntimeTaskChangedFile,
  RuntimeTaskResult,
  RuntimeTaskRole,
} from '../../state/taskRuntimeStore';

export const TERMINAL_TASK_STATUSES = new Set(['succeeded', 'failed', 'cancelled', 'recovery_required']);

const ROLE_LABELS: Record<RuntimeTaskRole, string> = {
  researcher: '调研员',
  engineer: '工程师',
  reviewer: '审阅员',
  analyst: '分析师',
  operator: '执行员',
  general: '协作者',
};

const EXECUTOR_LABELS: Record<string, string> = {
  pwcli: 'Workbench Agent',
  codex: 'Codex',
  qoder: 'Qoder',
  kimi: 'Kimi',
};

export type RuntimeStatusIcon =
  | 'queued'
  | 'starting'
  | 'running'
  | 'waiting'
  | 'attention'
  | 'materializing'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'recovery';

export interface RuntimeTaskPresentation {
  roleLabel: string;
  displayName?: string;
  executorLabel: string;
  statusLabel: string;
  statusIcon: RuntimeStatusIcon;
  documentId?: string;
  documentTitle: string;
  modelLabel?: string;
  effortLabel?: string;
  permissionLabel?: string;
  routingModeLabel?: string;
  routingReason?: string;
  totalTokens?: number;
  changedFiles: RuntimeTaskChangedFile[];
  avatarTone: number;
}

function objectValue(value: unknown): Record<string, unknown> | undefined {
  return value != null && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

function stringValue(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}

function taskResult(task: RuntimeTask): RuntimeTaskResult | undefined {
  return objectValue(task.result) as RuntimeTaskResult | undefined;
}

function taskMetadata(task: RuntimeTask): Record<string, unknown> | undefined {
  return objectValue(task.metadata);
}

function executorId(task: RuntimeTask): string {
  if (task.resolvedExecutorId) return task.resolvedExecutorId;
  if (task.backend) return task.backend;
  if (task.resolvedExecutorKind === 'internal_agent' || task.kind === 'child_agent') return 'pwcli';
  return task.kind === 'child_cli' ? 'cli' : '';
}

function statusPresentation(task: RuntimeTask): Pick<RuntimeTaskPresentation, 'statusLabel' | 'statusIcon'> {
  if (task.outputStatus === 'materializing' || (task.status === 'succeeded' && task.outputStatus === 'pending')) {
    return { statusLabel: '正在整理产出', statusIcon: 'materializing' };
  }
  if (task.outputStatus === 'failed' && task.status === 'succeeded') {
    return { statusLabel: '产出失败', statusIcon: 'failed' };
  }
  if (task.status === 'succeeded' && task.outputStatus === 'ready') {
    if (task.reviewStatus === 'pending') return { statusLabel: '待你审阅', statusIcon: 'attention' };
    if (task.reviewStatus === 'merge_required') return { statusLabel: '需要合并', statusIcon: 'attention' };
    if (task.reviewStatus === 'changes_requested') return { statusLabel: '等待修改', statusIcon: 'waiting' };
    if (task.reviewStatus === 'applied') {
      return task.access === 'mutating'
        ? { statusLabel: '已应用', statusIcon: 'completed' }
        : { statusLabel: '已确认', statusIcon: 'completed' };
    }
    if (task.reviewStatus === 'rejected') return { statusLabel: '已放弃', statusIcon: 'cancelled' };
  }
  switch (task.status) {
    case 'queued': return { statusLabel: '待开始', statusIcon: 'queued' };
    case 'leased': return { statusLabel: '正在启动', statusIcon: 'starting' };
    case 'running': return { statusLabel: '工作中', statusIcon: 'running' };
    case 'waiting_children': return { statusLabel: '等待协作者', statusIcon: 'waiting' };
    case 'waiting_user': return { statusLabel: '需要你处理', statusIcon: 'attention' };
    case 'waiting_configuration': return { statusLabel: '需要配置', statusIcon: 'attention' };
    case 'materializing': return { statusLabel: '正在整理产出', statusIcon: 'materializing' };
    case 'succeeded': return { statusLabel: '已完成', statusIcon: 'completed' };
    case 'failed':
      return task.outputStatus === 'ready'
        ? { statusLabel: '部分完成', statusIcon: 'failed' }
        : { statusLabel: '执行失败', statusIcon: 'failed' };
    case 'cancelled': return { statusLabel: '已取消', statusIcon: 'cancelled' };
    case 'recovery_required': return { statusLabel: '需要恢复', statusIcon: 'recovery' };
    default: return { statusLabel: task.status, statusIcon: 'waiting' };
  }
}

function avatarTone(seed: string): number {
  let hash = 0;
  for (let index = 0; index < seed.length; index += 1) {
    hash = ((hash << 5) - hash + seed.charCodeAt(index)) | 0;
  }
  return Math.abs(hash) % 6;
}

function tokenCount(task: RuntimeTask, result: RuntimeTaskResult | undefined): number | undefined {
  if (typeof result?.tokenUsage?.totalTokens === 'number') return result.tokenUsage.totalTokens;
  const metadata = taskMetadata(task);
  const tokenUsage = objectValue(metadata?.tokenUsage);
  return typeof tokenUsage?.totalTokens === 'number' ? tokenUsage.totalTokens : undefined;
}

export function presentRuntimeTask(task: RuntimeTask): RuntimeTaskPresentation {
  const result = taskResult(task);
  const document = objectValue(result?.document);
  const documentId = task.primaryDocumentId
    || stringValue(result?.primaryDocumentId)
    || stringValue(document?.id);
  const changedFiles = Array.isArray(result?.changedFiles)
    ? result.changedFiles.filter((file): file is RuntimeTaskChangedFile => (
      objectValue(file) != null && typeof file.path === 'string'
    ))
    : [];
  const selectedExecutor = executorId(task);
  return {
    roleLabel: task.roleLabel || (task.role ? ROLE_LABELS[task.role] : '协作者'),
    displayName: stringValue(task.displayName),
    executorLabel: EXECUTOR_LABELS[selectedExecutor] || selectedExecutor || '旧版委派',
    ...statusPresentation(task),
    documentId,
    documentTitle: task.deliverableTitle
      || stringValue(document?.title)
      || task.objective,
    modelLabel: task.resolvedModel || task.model,
    effortLabel: task.resolvedEffort,
    permissionLabel: task.resolvedPermissionMode,
    routingModeLabel: task.executorRequest
      ? (task.executorRequest === 'auto' ? '自动选择' : '显式指定')
      : undefined,
    routingReason: task.routingReason,
    totalTokens: tokenCount(task, result),
    changedFiles,
    avatarTone: avatarTone(task.avatarSeed || task.displayName || task.id),
  };
}

export function isTaskActive(task: RuntimeTask): boolean {
  return !TERMINAL_TASK_STATUSES.has(task.status)
    || task.outputStatus === 'pending'
    || task.outputStatus === 'materializing'
    || task.reviewStatus === 'pending'
    || task.reviewStatus === 'merge_required'
    || task.reviewStatus === 'changes_requested';
}

export function isTaskSettled(task: RuntimeTask): boolean {
  return TERMINAL_TASK_STATUSES.has(task.status)
    && task.outputStatus !== 'pending'
    && task.outputStatus !== 'materializing'
    && task.reviewStatus !== 'pending'
    && task.reviewStatus !== 'merge_required'
    && task.reviewStatus !== 'changes_requested';
}

export function runtimeTaskDepth(task: RuntimeTask, tasks: RuntimeTask[]): number {
  if (typeof task.depth === 'number') return Math.max(0, task.depth);
  const byId = new Map(tasks.map(item => [item.id, item]));
  const seen = new Set([task.id]);
  let depth = 0;
  let parentId = task.parentTaskId;
  while (parentId && !seen.has(parentId)) {
    seen.add(parentId);
    const parent = byId.get(parentId);
    if (!parent) break;
    depth += 1;
    parentId = parent.parentTaskId;
  }
  return depth;
}

export interface RuntimeTaskBatch {
  id: string;
  tasks: RuntimeTask[];
  visibleTasks: RuntimeTask[];
  deepTasks: RuntimeTask[];
  settledCount: number;
}

export function groupRuntimeTaskBatches(tasks: RuntimeTask[]): RuntimeTaskBatch[] {
  const groups = new Map<string, RuntimeTask[]>();
  tasks.forEach(task => {
    const group = groups.get(task.batchId);
    if (group) group.push(task);
    else groups.set(task.batchId, [task]);
  });
  return Array.from(groups, ([id, batchTasks]) => {
    const sorted = [...batchTasks].sort((left, right) => (
      Date.parse(left.createdAt) - Date.parse(right.createdAt)
    ));
    return {
      id,
      tasks: sorted,
      visibleTasks: sorted.filter(task => runtimeTaskDepth(task, tasks) <= 1),
      deepTasks: sorted.filter(task => runtimeTaskDepth(task, tasks) > 1),
      settledCount: sorted.filter(isTaskSettled).length,
    };
  }).sort((left, right) => (
    Date.parse(right.tasks[0]?.createdAt || '') - Date.parse(left.tasks[0]?.createdAt || '')
  ));
}
