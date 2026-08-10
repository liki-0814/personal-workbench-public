import { useEffect, useMemo, useState, type FormEvent } from 'react';
import {
  Ban,
  BarChart3,
  Bot,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CircleStop,
  Clock3,
  Code2,
  FileClock,
  FileText,
  GitCompare,
  LoaderCircle,
  MessageSquareMore,
  Network,
  RotateCcw,
  Search,
  ShieldCheck,
  Sparkles,
  Wrench,
  X,
} from 'lucide-react';
import type {
  RuntimeTask,
  RuntimeTaskEvent,
  RuntimeTaskExecutor,
  RuntimeTaskResult,
  RuntimeTaskReviewAction,
  RuntimeTaskRole,
} from '../../state/taskRuntimeStore';
import type { WorkspaceRef } from '@/domain/studio';
import { apiFetch } from '@/core/utils';
import { SelectField } from '@/shell';
import {
  groupRuntimeTaskBatches,
  isTaskActive,
  presentRuntimeTask,
  TERMINAL_TASK_STATUSES,
  type RuntimeStatusIcon,
} from './delegationPresentation';

function elapsed(task: RuntimeTask): string {
  const start = Date.parse(task.createdAt);
  const end = isTaskActive(task) ? Date.now() : Date.parse(task.updatedAt);
  if (!Number.isFinite(start) || !Number.isFinite(end)) return '';
  const seconds = Math.max(0, Math.round((end - start) / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

function RoleIcon({ role }: { role?: RuntimeTaskRole }) {
  const props = { size: 14, strokeWidth: 1.8 };
  switch (role) {
    case 'researcher': return <Search {...props} />;
    case 'engineer': return <Code2 {...props} />;
    case 'reviewer': return <ShieldCheck {...props} />;
    case 'analyst': return <BarChart3 {...props} />;
    case 'operator': return <Wrench {...props} />;
    case 'general': return <Sparkles {...props} />;
    default: return <Bot {...props} />;
  }
}

function StatusIcon({ kind }: { kind: RuntimeStatusIcon }) {
  const props = { size: 14, strokeWidth: 1.9, 'aria-hidden': true } as const;
  switch (kind) {
    case 'queued': return <Clock3 {...props} />;
    case 'starting':
    case 'running': return <LoaderCircle {...props} className="delegation-spin" />;
    case 'waiting': return <Clock3 {...props} />;
    case 'attention': return <CircleAlert {...props} />;
    case 'materializing': return <FileClock {...props} />;
    case 'completed': return <CheckCircle2 {...props} />;
    case 'failed': return <CircleAlert {...props} />;
    case 'cancelled': return <Ban {...props} />;
    case 'recovery': return <Wrench {...props} />;
  }
}

export interface DelegationCardProps {
  tasks: RuntimeTask[];
  taskEvents?: Record<string, RuntimeTaskEvent[]>;
  onCancel: (taskId: string) => Promise<void>;
  onRetry?: (
    taskId: string,
    executor?: Exclude<RuntimeTaskExecutor, 'auto'>,
  ) => Promise<void>;
  onResolveDecision?: (taskId: string, option: string, note?: string) => Promise<void>;
  onFollowUp: (taskId: string, objective: string) => Promise<void>;
  onReview?: (
    taskId: string,
    action: RuntimeTaskReviewAction,
    reviewRevision: number,
    note?: string,
    projection?: {
      taskUpdate?: RuntimeTaskResult['suggestedTaskUpdate'];
      memoryEntries?: RuntimeTaskResult['decisionCandidates'];
    },
  ) => Promise<void>;
  /** Legacy diff callback kept while the parent Chat surface migrates. */
  onOpenFile: (ref: WorkspaceRef) => void;
  onOpenDocument?: (documentId: string, task: RuntimeTask, title?: string) => void;
  onOpenDiff?: (ref: WorkspaceRef, task: RuntimeTask) => void;
  onOpenPanorama?: (batchId: string) => void;
  onOpenSettings?: (tab: 'ai' | 'integrations') => void;
  focusTaskId?: string;
}

const TASK_ACTIVITY_LABELS: Record<string, string> = {
  task_published: '任务已创建',
  task_leased: '执行器已接单',
  task_started: '开始执行',
  task_native_session_started: '已连接子 Agent 会话',
  task_waiting_children: '正在等待下级协作者',
  task_children_ready: '下级协作者已返回',
  task_child_result_buffered: '收到一份协作者结果',
  task_child_results_queued: '正在汇总协作者结果',
  task_waiting_user: '等待你的选择',
  task_waiting_configuration: '等待完成配置',
  task_follow_up_queued: '继续对话已排队',
  task_output_materializing: '正在整理交付物',
  task_output_ready: '交付物已就绪',
  task_output_failed: '交付物整理失败',
  task_completed: '任务已完成',
  task_failed: '任务执行失败',
  task_start_failed: '任务启动失败',
  task_cancelled: '任务已取消',
  task_recovery_required: '任务需要恢复',
};

function activityLabel(event: RuntimeTaskEvent): string {
  return TASK_ACTIVITY_LABELS[event.kind]
    || event.kind.replace(/^task_/, '').replace(/_/g, ' ');
}

function configurationGuidance(task: RuntimeTask): {
  title: string;
  detail?: string;
  settingsTab: 'ai' | 'integrations';
} {
  const executor = task.resolvedExecutorId || task.backend || task.executorRequest;
  const detail = task.error;
  const normalizedDetail = detail?.toLowerCase() ?? '';
  const label = RETRY_EXECUTOR_OPTIONS.find(item => item.value === executor)?.label || executor;
  if (normalizedDetail.includes('not enabled')) {
    return {
      title: `${label || '该执行器'} 已安装，但未在委派设置中启用`,
      detail,
      settingsTab: 'integrations',
    };
  }
  if (normalizedDetail.includes('does not support model')) {
    return {
      title: `${label || '该执行器'} 与指定模型不兼容`,
      detail,
      settingsTab: executor === 'pwcli' ? 'ai' : 'integrations',
    };
  }
  if (normalizedDetail.includes('git isolation')) {
    return {
      title: '当前项目无法创建安全的 Git 隔离工作区',
      detail,
      settingsTab: 'integrations',
    };
  }
  if (executor === 'pwcli' || !executor || executor === 'auto') {
    return {
      title: 'Workbench Agent 缺少可用的 AI Provider 或模型',
      detail,
      settingsTab: 'ai',
    };
  }
  return {
    title: `${label} 尚未安装、登录或启用`,
    detail,
    settingsTab: 'integrations',
  };
}

function reviewRevision(task: RuntimeTask): number {
  if (typeof task.reviewRevision === 'number') return task.reviewRevision;
  if (task.metadata && typeof task.metadata === 'object' && !Array.isArray(task.metadata)) {
    const value = (task.metadata as Record<string, unknown>).reviewRevision;
    if (typeof value === 'number') return value;
  }
  return 0;
}

interface TaskDocumentSummary {
  id: string;
  title: string;
  kind: 'html' | 'markdown';
  revision: number;
  updatedAt: string;
}

const RETRY_EXECUTOR_OPTIONS = [
  { value: 'pwcli', label: 'Workbench Agent' },
  { value: 'codex', label: 'Codex' },
  { value: 'qoder', label: 'Qoder' },
  { value: 'kimi', label: 'Kimi' },
];

function TaskDocumentList({
  task,
  primaryDocumentId,
  onOpenDocument,
}: {
  task: RuntimeTask;
  primaryDocumentId: string;
  onOpenDocument: (documentId: string, task: RuntimeTask, title?: string) => void;
}) {
  const [documents, setDocuments] = useState<TaskDocumentSummary[]>();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let alive = true;
    void apiFetch<TaskDocumentSummary[]>(`/api/agent/runtime/tasks/${encodeURIComponent(task.id)}/documents`)
      .then(items => {
        if (alive) setDocuments(items.filter(item => item.id !== primaryDocumentId));
      })
      .catch(() => {
        if (alive) setDocuments([]);
      });
    return () => { alive = false; };
  }, [primaryDocumentId, task.id]);

  if (!documents?.length) return null;
  return (
    <div className="delegation-document-list">
      <button type="button" onClick={() => setOpen(value => !value)} aria-expanded={open}>
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        另有 {documents.length} 份文档
      </button>
      {open && (
        <div>
          {documents.map(document => (
            <button
              type="button"
              key={document.id}
              onClick={() => onOpenDocument(document.id, task, document.title)}
              title={`阅读：${document.title}`}
            >
              <FileText size={12} />
              <span>{document.title}</span>
              <small>r{document.revision}</small>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export function DelegateBatchCard({
  tasks,
  taskEvents = {},
  onCancel,
  onRetry,
  onResolveDecision,
  onFollowUp,
  onReview,
  onOpenFile,
  onOpenDocument,
  onOpenDiff,
  onOpenPanorama,
  onOpenSettings,
  focusTaskId,
}: DelegationCardProps) {
  const [expanded, setExpanded] = useState(false);
  const [detailTaskIds, setDetailTaskIds] = useState<Set<string>>(() => new Set());
  const [followUpTaskId, setFollowUpTaskId] = useState<string>();
  const [followUpDraft, setFollowUpDraft] = useState('');
  const [followUpPending, setFollowUpPending] = useState(false);
  const [followUpError, setFollowUpError] = useState('');
  const [reviewTaskId, setReviewTaskId] = useState<string>();
  const [reviewDraft, setReviewDraft] = useState('');
  const [reviewPendingTaskId, setReviewPendingTaskId] = useState<string>();
  const [retryPendingTaskId, setRetryPendingTaskId] = useState<string>();
  const [cancelPendingTaskId, setCancelPendingTaskId] = useState<string>();
  const [retryPickerTaskId, setRetryPickerTaskId] = useState<string>();
  const [retryExecutor, setRetryExecutor] = useState<Exclude<RuntimeTaskExecutor, 'auto'>>('codex');
  const [decisionPendingKey, setDecisionPendingKey] = useState<string>();
  const [reviewErrors, setReviewErrors] = useState<Record<string, string>>({});
  const [applyTaskId, setApplyTaskId] = useState<string>();
  const [markWholeTask, setMarkWholeTask] = useState(false);
  const [progressDraft, setProgressDraft] = useState('');
  const [decisionDraft, setDecisionDraft] = useState('');
  const [selectedDecisionIds, setSelectedDecisionIds] = useState<Set<string>>(() => new Set());
  const batches = useMemo(() => groupRuntimeTaskBatches(tasks), [tasks]);
  const activeCount = useMemo(() => tasks.filter(isTaskActive).length, [tasks]);
  const settledCount = tasks.length - activeCount;

  useEffect(() => {
    if (focusTaskId) setExpanded(true);
  }, [focusTaskId]);

  if (tasks.length === 0) return null;

  const toggleDetails = (taskId: string) => {
    setDetailTaskIds(current => {
      const next = new Set(current);
      if (next.has(taskId)) next.delete(taskId);
      else next.add(taskId);
      return next;
    });
  };

  const openFollowUp = (taskId: string) => {
    setFollowUpTaskId(taskId);
    setFollowUpDraft('');
    setFollowUpError('');
  };

  const closeFollowUp = () => {
    if (followUpPending) return;
    setFollowUpTaskId(undefined);
    setFollowUpDraft('');
    setFollowUpError('');
  };

  const submitFollowUp = async (event: FormEvent<HTMLFormElement>, taskId: string) => {
    event.preventDefault();
    const objective = followUpDraft.trim();
    if (!objective || followUpPending) return;
    setFollowUpPending(true);
    setFollowUpError('');
    try {
      await onFollowUp(taskId, objective);
      setFollowUpTaskId(undefined);
      setFollowUpDraft('');
    } catch {
      setFollowUpError('发送失败，请重试');
    } finally {
      setFollowUpPending(false);
    }
  };

  const submitReview = async (
    task: RuntimeTask,
    action: RuntimeTaskReviewAction,
    note?: string,
    projection?: {
      taskUpdate?: RuntimeTaskResult['suggestedTaskUpdate'];
      memoryEntries?: RuntimeTaskResult['decisionCandidates'];
    },
  ) => {
    if (!onReview || reviewPendingTaskId) return;
    setReviewPendingTaskId(task.id);
    setReviewErrors(current => ({ ...current, [task.id]: '' }));
    try {
      await onReview(task.id, action, reviewRevision(task), note, projection);
      setReviewTaskId(undefined);
      setApplyTaskId(undefined);
      setReviewDraft('');
      setProgressDraft('');
      setDecisionDraft('');
    } catch (reason) {
      setReviewErrors(current => ({
        ...current,
        [task.id]: reason instanceof Error ? reason.message : '审阅操作失败，请重试',
      }));
    } finally {
      setReviewPendingTaskId(undefined);
    }
  };

  const cancelTask = async (task: RuntimeTask) => {
    if (cancelPendingTaskId) return;
    setCancelPendingTaskId(task.id);
    setReviewErrors(current => ({ ...current, [task.id]: '' }));
    try {
      await onCancel(task.id);
    } catch (reason) {
      setReviewErrors(current => ({
        ...current,
        [task.id]: reason instanceof Error ? reason.message : '取消任务失败，请重试',
      }));
    } finally {
      setCancelPendingTaskId(undefined);
    }
  };

  const retryTask = async (
    task: RuntimeTask,
    executor?: Exclude<RuntimeTaskExecutor, 'auto'>,
  ) => {
    if (!onRetry || retryPendingTaskId) return;
    setRetryPendingTaskId(task.id);
    setReviewErrors(current => ({ ...current, [task.id]: '' }));
    try {
      if (executor) await onRetry(task.id, executor);
      else await onRetry(task.id);
      setRetryPickerTaskId(undefined);
    } catch (reason) {
      setReviewErrors(current => ({
        ...current,
        [task.id]: reason instanceof Error ? reason.message : '重试失败，请检查执行器配置',
      }));
    } finally {
      setRetryPendingTaskId(undefined);
    }
  };

  const openRetryPicker = (task: RuntimeTask) => {
    const current = task.resolvedExecutorId;
    const alternative = RETRY_EXECUTOR_OPTIONS.find(option => option.value !== current)?.value
      ?? 'pwcli';
    setRetryExecutor(alternative as Exclude<RuntimeTaskExecutor, 'auto'>);
    setRetryPickerTaskId(task.id);
  };

  const resolveDecision = async (task: RuntimeTask, option: string) => {
    if (!onResolveDecision || decisionPendingKey) return;
    const key = `${task.id}:${option}`;
    setDecisionPendingKey(key);
    setReviewErrors(current => ({ ...current, [task.id]: '' }));
    try {
      await onResolveDecision(task.id, option);
    } catch (reason) {
      setReviewErrors(current => ({
        ...current,
        [task.id]: reason instanceof Error ? reason.message : '提交选择失败，请重试',
      }));
    } finally {
      setDecisionPendingKey(undefined);
    }
  };

  return (
    <section className="delegation-card" aria-label="异步委派">
      <div className="delegation-card-header">
        <button
          type="button"
          className="delegation-card-summary"
          onClick={() => setExpanded(value => !value)}
          aria-expanded={expanded || activeCount > 0}
        >
          {(expanded || activeCount > 0) ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          <span>{tasks.length} 位协作者</span>
          {activeCount > 0 && <span className="delegation-card-active">{activeCount} 位进行中</span>}
          <span className="delegation-card-count">{settledCount}/{tasks.length} 已完成</span>
        </button>
        {onOpenPanorama && batches[0] && (
          <button
            type="button"
            className="delegation-panorama-action"
            onClick={() => onOpenPanorama(batches[0].id)}
            title="打开最新批次协作全景图"
          >
            <Network size={13} />
            协作全景图
          </button>
        )}
      </div>

      {(expanded || activeCount > 0) && (
        <div className="delegation-card-list">
          {batches.map(batch => (
            <div className="delegation-batch" key={batch.id}>
              {batches.length > 1 && (
                <div className="delegation-batch-summary">
                  <span>{batch.tasks.length} 位协作者</span>
                  <span>{batch.settledCount}/{batch.tasks.length} 已完成</span>
                  {onOpenPanorama && (
                    <button type="button" onClick={() => onOpenPanorama(batch.id)} title="打开本批次协作全景图">
                      <Network size={12} />
                      全景图
                    </button>
                  )}
                </div>
              )}
              {batch.visibleTasks.map(task => {
                const view = presentRuntimeTask(task);
                const result = task.result && typeof task.result === 'object' && !Array.isArray(task.result)
                  ? task.result as RuntimeTaskResult
                  : undefined;
                const decisionCandidates = result?.decisionCandidates ?? [];
                const decisionQuestion = task.status === 'waiting_user' && typeof result?.question === 'string'
                  ? result.question.trim()
                  : '';
                const decisionOptions = task.status === 'waiting_user' && Array.isArray(result?.options)
                  ? result.options.filter((option): option is string => typeof option === 'string' && Boolean(option.trim()))
                  : [];
                const detailsOpen = detailTaskIds.has(task.id);
                const events = taskEvents[task.id] ?? [];
                const configuration = task.status === 'waiting_configuration'
                  ? configurationGuidance(task)
                  : undefined;
                const documentConfirmationReady = task.access !== 'mutating'
                  && task.status === 'succeeded'
                  && task.outputStatus === 'ready'
                  && task.reviewStatus !== 'applied';
                const identity = view.displayName
                  ? `${view.roleLabel} ${view.displayName}`
                  : view.roleLabel;
                return (
                  <article
                    id={`delegation-task-${task.id}`}
                    className={`delegation-card-item ${focusTaskId === task.id ? 'is-focused' : ''}`}
                    key={task.id}
                    tabIndex={focusTaskId === task.id ? -1 : undefined}
                  >
                    <div className="delegation-identity-row">
                      <span className={`delegation-avatar is-tone-${view.avatarTone}`} aria-hidden="true">
                        <RoleIcon role={task.role} />
                      </span>
                      <span className="delegation-identity" title={identity}>{identity}</span>
                      <span className="delegation-executor">{view.executorLabel}</span>
                      <span className={`delegation-state is-${view.statusIcon}`} aria-label={`状态：${view.statusLabel}`}>
                        <StatusIcon kind={view.statusIcon} />
                        <span>{view.statusLabel}</span>
                      </span>
                    </div>

                    <div className="delegation-deliverable-row">
                      <span className="delegation-branch" aria-hidden="true">└─</span>
                      {view.documentId && onOpenDocument ? (
                        <button
                          type="button"
                          className="delegation-deliverable"
                          onClick={() => onOpenDocument(view.documentId!, task)}
                          title={`阅读：${view.documentTitle}`}
                        >
                          <FileText size={13} />
                          <span>{view.documentTitle}</span>
                        </button>
                      ) : (
                        <span className="delegation-deliverable is-static" title={view.documentTitle}>
                          <span>{view.documentTitle}</span>
                        </span>
                      )}
                      <button
                        type="button"
                        className="delegation-action is-labeled"
                        onClick={() => toggleDetails(task.id)}
                        title={detailsOpen ? '收起进度' : '查看进度'}
                        aria-expanded={detailsOpen}
                      >
                        {detailsOpen ? <ChevronDown size={14} /> : <MessageSquareMore size={14} />}
                        <span>{detailsOpen ? '收起' : '查看进度'}</span>
                      </button>
                      {onRetry && ['waiting_configuration', 'failed', 'recovery_required', 'cancelled'].includes(task.status) && (
                        <>
                          <button
                            type="button"
                            className="delegation-action"
                            disabled={retryPendingTaskId === task.id}
                            onClick={() => void retryTask(task)}
                            title={task.status === 'waiting_configuration' ? '配置完成后重试' : '使用原执行器重试'}
                          >
                            <RotateCcw size={14} className={retryPendingTaskId === task.id ? 'delegation-spin' : undefined} />
                          </button>
                          <button
                            type="button"
                            className="delegation-action"
                            disabled={retryPendingTaskId === task.id}
                            onClick={() => openRetryPicker(task)}
                            title="选择其他执行器重试"
                          >
                            <ChevronDown size={14} />
                          </button>
                        </>
                      )}
                      {['succeeded', 'failed'].includes(task.status) && (
                        <button
                          type="button"
                          className="delegation-action is-labeled"
                          onClick={() => openFollowUp(task.id)}
                          title="追加要求"
                        >
                          <MessageSquareMore size={14} />
                          <span>继续对话</span>
                        </button>
                      )}
                      {!TERMINAL_TASK_STATUSES.has(task.status) && (
                        <button
                          type="button"
                          className="delegation-action"
                          disabled={Boolean(cancelPendingTaskId)}
                          onClick={() => void cancelTask(task)}
                          title={cancelPendingTaskId === task.id ? '正在取消任务' : '取消任务'}
                          aria-label={cancelPendingTaskId === task.id ? '正在取消任务' : '取消任务'}
                        >
                          {cancelPendingTaskId === task.id
                            ? <LoaderCircle size={14} className="delegation-spin" />
                            : <CircleStop size={14} />}
                        </button>
                      )}
                    </div>

                    {detailsOpen && (
                      <>
                        <div className="delegation-details">
                          <strong className="delegation-progress-heading">正在做什么</strong>
                          <span className="delegation-current-step">{view.statusLabel} · {view.documentTitle}</span>
                          <span>{view.modelLabel || '默认模型'}</span>
                          {view.effortLabel && <span>推理 {view.effortLabel}</span>}
                          {view.permissionLabel && <span>权限 {view.permissionLabel}</span>}
                          {elapsed(task) && <span>耗时 {elapsed(task)}</span>}
                          {view.totalTokens != null && <span>{view.totalTokens.toLocaleString()} tokens</span>}
                          {view.routingModeLabel && <span>{view.routingModeLabel}</span>}
                          {view.routingReason && <span className="delegation-routing-reason">{view.routingReason}</span>}
                          {events.length > 0 && (
                            <ol className="delegation-activity-list" aria-label="任务进度记录">
                              {events.slice(-5).map(event => (
                                <li key={event.eventId}>
                                  <time>{new Date(event.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time>
                                  <span>{activityLabel(event)}</span>
                                </li>
                              ))}
                            </ol>
                          )}
                        </div>
                        {view.documentId && onOpenDocument && (
                          <TaskDocumentList
                            task={task}
                            primaryDocumentId={view.documentId}
                            onOpenDocument={onOpenDocument}
                          />
                        )}
                      </>
                    )}

                    {configuration && (
                      <div className="delegation-configuration" role="alert">
                        <CircleAlert size={15} />
                        <div>
                          <strong>{configuration.title}</strong>
                          {configuration.detail && <span>{configuration.detail}</span>}
                        </div>
                        {onOpenSettings && (
                          <button type="button" onClick={() => onOpenSettings(configuration.settingsTab)}>
                            打开对应设置
                          </button>
                        )}
                      </div>
                    )}

                    {retryPickerTaskId === task.id && onRetry && (
                      <div className="delegation-retry-picker" role="group" aria-label="换执行器重试">
                        <span>换执行器重试</span>
                        <SelectField
                          value={retryExecutor}
                          options={RETRY_EXECUTOR_OPTIONS}
                          onValueChange={value => setRetryExecutor(value as Exclude<RuntimeTaskExecutor, 'auto'>)}
                          density="compact"
                          ariaLabel="选择重试执行器"
                          disabled={retryPendingTaskId === task.id}
                          menuMinWidth={180}
                        />
                        <button
                          type="button"
                          className="is-primary"
                          disabled={retryPendingTaskId === task.id}
                          onClick={() => void retryTask(task, retryExecutor)}
                        >
                          重试
                        </button>
                        <button
                          type="button"
                          disabled={retryPendingTaskId === task.id}
                          onClick={() => setRetryPickerTaskId(undefined)}
                        >
                          取消
                        </button>
                      </div>
                    )}

                    {decisionQuestion && decisionOptions.length > 0 && onResolveDecision && (
                      <div className="delegation-decision-request" role="group" aria-label="协作者需要你的选择">
                        <strong>{decisionQuestion}</strong>
                        <div>
                          {decisionOptions.map(option => {
                            const key = `${task.id}:${option}`;
                            return (
                              <button
                                type="button"
                                key={option}
                                disabled={Boolean(decisionPendingKey)}
                                onClick={() => void resolveDecision(task, option)}
                              >
                                {decisionPendingKey === key && <LoaderCircle size={12} className="delegation-spin" />}
                                {option}
                              </button>
                            );
                          })}
                        </div>
                      </div>
                    )}

                    {view.changedFiles.length > 0 && (
                      <div className="delegation-file-links">
                        {view.changedFiles.map(file => {
                          const ref: WorkspaceRef = {
                            path: file.path,
                            kind: 'diff',
                            taskId: task.id,
                            workspaceRoot: task.cwd,
                            reviewRevision: task.reviewRevision,
                            inlinePatch: file.patch,
                          };
                          return (
                            <button
                              type="button"
                              key={file.path}
                              onClick={() => (onOpenDiff ? onOpenDiff(ref, task) : onOpenFile(ref))}
                            >
                              <GitCompare size={12} />
                              {file.path.split('/').pop() || file.path} 文件审阅
                            </button>
                          );
                        })}
                      </div>
                    )}

                    {onReview && (task.reviewStatus === 'pending' || task.reviewStatus === 'merge_required') && (
                      <div className="delegation-review-actions" aria-label="改动审阅">
                        {task.reviewStatus === 'pending' && (
                          <button
                            type="button"
                            className="is-primary"
                            disabled={reviewPendingTaskId === task.id}
                            onClick={() => {
                              setApplyTaskId(task.id);
                              setMarkWholeTask(false);
                              setProgressDraft(result?.suggestedTaskUpdate?.progress?.toString() ?? '');
                              setDecisionDraft('');
                              setSelectedDecisionIds(new Set());
                            }}
                          >
                            应用改动
                          </button>
                        )}
                        <button
                          type="button"
                          disabled={reviewPendingTaskId === task.id}
                          onClick={() => {
                            setReviewTaskId(task.id);
                            setReviewDraft('');
                          }}
                        >
                          要求修改
                        </button>
                        <button
                          type="button"
                          disabled={reviewPendingTaskId === task.id}
                          onClick={() => void submitReview(task, 'reject')}
                        >
                          放弃改动
                        </button>
                      </div>
                    )}

                    {onReview && documentConfirmationReady && (
                      <div className="delegation-review-actions" aria-label="文档产出确认">
                        <button
                          type="button"
                          className="is-primary"
                          disabled={reviewPendingTaskId === task.id}
                          onClick={() => {
                            setApplyTaskId(task.id);
                            setMarkWholeTask(false);
                            setProgressDraft(result?.suggestedTaskUpdate?.progress?.toString() ?? '');
                            setDecisionDraft('');
                            setSelectedDecisionIds(new Set());
                          }}
                        >
                          确认完成
                        </button>
                      </div>
                    )}

                    {applyTaskId === task.id && (
                      <div
                        className="delegation-apply-confirm"
                        role="group"
                        aria-label={task.access === 'mutating' ? '确认应用改动' : '确认文档产出'}
                      >
                        <strong>{task.access === 'mutating'
                          ? `确认应用 ${view.changedFiles.length} 个文件`
                          : '确认本次产出并更新任务'}</strong>
                        {task.access === 'mutating' && view.changedFiles.length > 0 && (
                          <ul>
                            {view.changedFiles.map(file => <li key={file.path}>{file.path}</li>)}
                          </ul>
                        )}
                        {task.workItemId && (
                          <div className="delegation-task-update-fields">
                            <label>
                              <span>更新任务进度</span>
                              <input
                                type="number"
                                min="0"
                                max="100"
                                inputMode="numeric"
                                value={progressDraft}
                                onChange={event => setProgressDraft(event.target.value)}
                                placeholder="保持不变"
                              />
                              <small>%</small>
                            </label>
                            <label>
                              <input
                                type="checkbox"
                                checked={markWholeTask}
                                onChange={event => setMarkWholeTask(event.target.checked)}
                              />
                              标记整个任务完成
                            </label>
                          </div>
                        )}
                        {decisionCandidates.length > 0 && (
                          <fieldset>
                            <legend>保存关键决策（默认不选）</legend>
                            {decisionCandidates.map((candidate, index) => {
                              const id = candidate.id || `${task.id}:${index}`;
                              return (
                                <label key={id}>
                                  <input
                                    type="checkbox"
                                    checked={selectedDecisionIds.has(id)}
                                    onChange={event => setSelectedDecisionIds(current => {
                                      const next = new Set(current);
                                      if (event.target.checked) next.add(id);
                                      else next.delete(id);
                                      return next;
                                    })}
                                  />
                                  {candidate.title || candidate.summary || candidate.content.slice(0, 80)}
                                </label>
                              );
                            })}
                          </fieldset>
                        )}
                        <label className="delegation-decision-draft">
                          <span>补充关键决策（可选）</span>
                          <textarea
                            value={decisionDraft}
                            onChange={event => setDecisionDraft(event.target.value)}
                            placeholder="例如：后续统一使用 RuntimeTask 作为委派事实源"
                            rows={2}
                          />
                        </label>
                        <div className="delegation-apply-confirm-actions">
                          <button
                            type="button"
                            className="is-primary"
                            disabled={reviewPendingTaskId === task.id}
                            onClick={() => {
                              const suggested = result?.suggestedTaskUpdate;
                              const parsedProgress = progressDraft.trim() === ''
                                ? undefined
                                : Math.max(0, Math.min(100, Number(progressDraft)));
                              const taskUpdate = (task.workItemId || suggested)
                                ? {
                                    ...suggested,
                                    todoId: suggested?.todoId || task.workItemId,
                                    ...(typeof parsedProgress === 'number' && Number.isFinite(parsedProgress)
                                      ? { progress: parsedProgress }
                                      : {}),
                                    markComplete: markWholeTask,
                                  }
                                : undefined;
                              const memoryEntries = decisionCandidates.filter((candidate, index) => (
                                selectedDecisionIds.has(candidate.id || `${task.id}:${index}`)
                              ));
                              if (decisionDraft.trim()) {
                                memoryEntries.push({
                                  title: decisionDraft.trim().split('\n')[0].slice(0, 80),
                                  content: decisionDraft.trim(),
                                });
                              }
                              void submitReview(task, task.access === 'mutating' ? 'apply' : 'confirm', undefined, {
                                taskUpdate,
                                memoryEntries,
                              });
                            }}
                          >
                            {task.access === 'mutating' ? '应用并确认' : '确认完成'}
                          </button>
                          <button
                            type="button"
                            disabled={reviewPendingTaskId === task.id}
                            onClick={() => setApplyTaskId(undefined)}
                          >
                            取消
                          </button>
                        </div>
                      </div>
                    )}

                    {reviewTaskId === task.id && (
                      <form
                        className="delegation-follow-up delegation-review-form"
                        onSubmit={event => {
                          event.preventDefault();
                          const note = reviewDraft.trim();
                          if (note) void submitReview(task, 'request_changes', note);
                        }}
                      >
                        <label htmlFor={`delegation-review-${task.id}`}>修改要求</label>
                        <div>
                          <input
                            id={`delegation-review-${task.id}`}
                            value={reviewDraft}
                            onChange={event => setReviewDraft(event.target.value)}
                            onKeyDown={event => {
                              if (event.key === 'Escape' && !reviewPendingTaskId) setReviewTaskId(undefined);
                            }}
                            placeholder="说明需要调整的内容"
                            autoFocus
                            disabled={reviewPendingTaskId === task.id}
                          />
                          <button type="submit" disabled={!reviewDraft.trim() || reviewPendingTaskId === task.id} title="发送修改要求">
                            <Check size={14} />
                          </button>
                          <button type="button" onClick={() => setReviewTaskId(undefined)} disabled={reviewPendingTaskId === task.id} title="取消">
                            <X size={14} />
                          </button>
                        </div>
                      </form>
                    )}

                    {reviewErrors[task.id] && <div className="delegation-review-error" role="alert">{reviewErrors[task.id]}</div>}

                    {followUpTaskId === task.id && (
                      <form className="delegation-follow-up" onSubmit={event => void submitFollowUp(event, task.id)}>
                        <label htmlFor={`delegation-follow-up-${task.id}`}>追加要求</label>
                        <div>
                          <input
                            id={`delegation-follow-up-${task.id}`}
                            value={followUpDraft}
                            onChange={event => setFollowUpDraft(event.target.value)}
                            onKeyDown={event => {
                              if (event.key === 'Escape') closeFollowUp();
                            }}
                            placeholder="说明需要补充或修改的内容"
                            autoFocus
                            disabled={followUpPending}
                          />
                          <button type="submit" disabled={!followUpDraft.trim() || followUpPending} title="发送追加要求">
                            <Check size={14} />
                          </button>
                          <button type="button" onClick={closeFollowUp} disabled={followUpPending} title="取消">
                            <X size={14} />
                          </button>
                        </div>
                        {followUpError && <span role="alert">{followUpError}</span>}
                      </form>
                    )}
                  </article>
                );
              })}

              {expanded && batch.deepTasks.length > 0 && (
                <div className="delegation-deep-tasks">
                  <span>更深层协作</span>
                  {batch.deepTasks.map(task => {
                    const view = presentRuntimeTask(task);
                    const detailsOpen = detailTaskIds.has(task.id);
                    const events = taskEvents[task.id] ?? [];
                    const configuration = task.status === 'waiting_configuration'
                      ? configurationGuidance(task)
                      : undefined;
                    return (
                      <div
                        id={`delegation-task-${task.id}`}
                        key={task.id}
                        className={focusTaskId === task.id ? 'is-focused' : undefined}
                        tabIndex={focusTaskId === task.id ? -1 : undefined}
                      >
                        <span>{view.displayName ? `${view.roleLabel} ${view.displayName}` : view.roleLabel}</span>
                        {view.documentId && onOpenDocument ? (
                          <button
                            type="button"
                            className="delegation-deep-document"
                            onClick={() => onOpenDocument(view.documentId!, task)}
                          >
                            {view.documentTitle}
                          </button>
                        ) : <span>{view.documentTitle}</span>}
                        <div className="delegation-deep-actions">
                          <span className={`delegation-state is-${view.statusIcon}`}>
                            <StatusIcon kind={view.statusIcon} />
                            {view.statusLabel}
                          </span>
                          <button type="button" onClick={() => toggleDetails(task.id)}>
                            {detailsOpen ? '收起' : '查看进度'}
                          </button>
                        </div>
                        {detailsOpen && (
                          <div className="delegation-deep-details">
                            <strong>正在做什么</strong>
                            <span>{view.statusLabel} · {task.objective}</span>
                            {events.slice(-5).map(event => (
                              <span key={event.eventId}>{activityLabel(event)}</span>
                            ))}
                          </div>
                        )}
                        {configuration && (
                          <div className="delegation-deep-configuration" role="alert">
                            <strong>{configuration.title}</strong>
                            {configuration.detail && <span>{configuration.detail}</span>}
                            {onOpenSettings && (
                              <button type="button" onClick={() => onOpenSettings(configuration.settingsTab)}>
                                打开对应设置
                              </button>
                            )}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

export default DelegateBatchCard;
