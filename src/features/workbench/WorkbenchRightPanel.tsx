import { useMemo, useRef, useState } from 'react';
import { Check, CircleAlert, FileText, GitCompare, LoaderCircle, MessageSquareMore, Plus, Trash2 } from 'lucide-react';
import type { HabitItem } from '@/domain/habit';
import type { TodoItem } from '@/domain/todo';
import type { DayPlanItem } from '@/domain/day-plan';
import { JobsPanel } from '@/domain/jobs';
import {
  presentRuntimeTask,
  type WorkItem,
  type LocalProject,
  type WorkItemNavigationTarget,
  type WorkItemProjection,
} from '@/domain/chat';
import CaptureWorkItemModal from './CaptureWorkItemModal';
import type { WorkItemCaptureRequest } from './workItemCapture';

export type WorkbenchView = 'inbox' | 'objectives' | 'day_plan' | 'habits' | 'jobs';

interface Props {
  activeView: WorkbenchView;
  onActiveViewChange: (view: WorkbenchView) => void;
  habits: HabitItem[];
  todos: TodoItem[];
  dayPlans: DayPlanItem[];
  isTodayDue: (habit: HabitItem) => boolean;
  habitPanel: React.ReactNode;
  objectivePanel: React.ReactNode;
  dayPlanPanel: React.ReactNode;
  onAskAiForJob?: () => void;
  workItems?: WorkItemProjection;
  onOpenWorkItem?: (item: WorkItem, target: WorkItemNavigationTarget) => void;
  onResolveAttention?: (attentionId: string) => Promise<void>;
  onDeleteAttention?: (attentionId: string) => Promise<void>;
  projects: LocalProject[];
  onCaptureWorkItem: (request: WorkItemCaptureRequest) => Promise<void>;
}

const EMPTY_WORK_ITEMS: WorkItemProjection = {
  attention: [],
  running: [],
  continueWork: [],
  badges: { attention: 0, running: 0, continueWork: 0, total: 0 },
  hasUnresolvedAttention: false,
};

const ACKNOWLEDGEABLE_ATTENTION_KINDS = new Set([
  'task_failed',
  'output_failed',
  'paused_callback',
]);

function getTodayLabel(): string {
  return new Intl.DateTimeFormat('zh-CN', {
    month: 'long',
    day: 'numeric',
    weekday: 'short',
  }).format(new Date());
}

export default function WorkbenchRightPanel({
  activeView,
  onActiveViewChange,
  habits,
  todos,
  dayPlans,
  isTodayDue,
  habitPanel,
  objectivePanel,
  dayPlanPanel,
  onAskAiForJob,
  workItems = EMPTY_WORK_ITEMS,
  onOpenWorkItem,
  onResolveAttention,
  onDeleteAttention,
  projects,
  onCaptureWorkItem,
}: Props) {
  const [captureOpen, setCaptureOpen] = useState(false);
  const pendingTodoCount = useMemo(
    () => todos.filter((t) => !t.completed).length,
    [todos],
  );

  const inProgressCount = useMemo(
    () => todos.filter((t) => !t.completed && t.status === 'in_progress').length,
    [todos],
  );

  const intraDayPlanCount = useMemo(
    () => dayPlans.length,
    [dayPlans],
  );

  const dueHabitCount = useMemo(
    () => habits.filter(isTodayDue).length,
    [habits, isTodayDue],
  );

  const tabs: { id: WorkbenchView; label: string; count?: number }[] = [
    { id: 'inbox', label: '收件箱', count: workItems.badges.attention || undefined },
    { id: 'objectives', label: '目标与任务', count: pendingTodoCount },
    { id: 'day_plan', label: '日内计划', count: intraDayPlanCount },
    { id: 'habits', label: '习惯', count: dueHabitCount },
    { id: 'jobs', label: '调度' },
  ];

  return (
    <>
      <section className={`workbench-view-${activeView} flex h-full min-h-0 min-w-0 flex-col`}>
      <div className="workbench-heading flex flex-wrap items-center justify-between gap-3 px-1 pb-4">
        <div className="min-w-0">
          <div className="text-[13px] font-medium tracking-[-0.01em] text-[#7A818C] dark:text-[#8D96A3]">
            {getTodayLabel()}
          </div>
          <div className="mt-1 flex items-baseline gap-2">
            <h2 className="text-[22px] font-semibold tracking-[-0.025em] text-[#171A1F] dark:text-[#E9ECF1]">
              今日工作
            </h2>
            <span className="text-[13px] font-medium tabular-nums text-[#69717D] dark:text-[#98A1AD]">
              {pendingTodoCount} 项待处理
              {inProgressCount > 0 ? ` · ${inProgressCount} 项进行中` : ''}
            </span>
          </div>
        </div>
        <button type="button" className="workbench-capture-button" onClick={() => setCaptureOpen(true)}>
          <Plus size={15} aria-hidden="true" />
          捕获事项
        </button>
      </div>

      <div className="workbench-tabs flex shrink-0 gap-1 overflow-x-auto" role="tablist" aria-label="工作台视图">
        {tabs.map((tab, index) => (
          <button
            key={tab.id}
            id={`workbench-tab-${tab.id}`}
            type="button"
            role="tab"
            aria-selected={activeView === tab.id}
            aria-controls={`workbench-panel-${tab.id}`}
            tabIndex={activeView === tab.id ? 0 : -1}
            onClick={() => onActiveViewChange(tab.id)}
            onKeyDown={event => {
              let nextIndex = index;
              if (event.key === 'ArrowRight') nextIndex = (index + 1) % tabs.length;
              else if (event.key === 'ArrowLeft') nextIndex = (index - 1 + tabs.length) % tabs.length;
              else if (event.key === 'Home') nextIndex = 0;
              else if (event.key === 'End') nextIndex = tabs.length - 1;
              else return;
              event.preventDefault();
              onActiveViewChange(tabs[nextIndex].id);
              event.currentTarget.parentElement
                ?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[nextIndex]
                ?.focus();
            }}
            className={`relative flex h-11 shrink-0 items-center gap-1.5 px-3 text-[13px] font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[#8F98A5]/50 ${
              activeView === tab.id
                ? 'text-[#2448C5] after:absolute after:inset-x-2 after:bottom-0 after:h-0.5 after:bg-[#3559D6] dark:text-[#9DB0FF]'
                : 'text-[#69717D] hover:text-[#171A1F] dark:text-[#98A1AD] dark:hover:text-[#E9ECF1]'
            }`}
          >
            {tab.label}
            {tab.count != null && tab.count > 0 && (
              <span className="font-mono text-[10px] tabular-nums opacity-75">{tab.count}</span>
            )}
          </button>
        ))}
      </div>

      <div
        id={`workbench-panel-${activeView}`}
        role="tabpanel"
        aria-labelledby={`workbench-tab-${activeView}`}
        className={`relative min-h-0 flex-1 pt-3 ${activeView === 'objectives' ? 'overflow-visible' : 'overflow-y-auto'}`}
      >
        {activeView === 'habits' && habitPanel}
        {activeView === 'jobs' && <JobsPanel onAskAi={onAskAiForJob} />}
        {activeView === 'objectives' && objectivePanel}
        {activeView === 'day_plan' && dayPlanPanel}
        {activeView === 'inbox' && (
          <WorkInbox
            projection={workItems}
            onOpenItem={onOpenWorkItem}
            onResolveAttention={onResolveAttention}
            onDeleteAttention={onDeleteAttention}
          />
        )}
      </div>
      </section>
      <CaptureWorkItemModal
        open={captureOpen}
        projects={projects}
        onClose={() => setCaptureOpen(false)}
        onCapture={onCaptureWorkItem}
      />
    </>
  );
}

function WorkInbox({
  projection,
  onOpenItem,
  onResolveAttention,
  onDeleteAttention,
}: {
  projection: WorkItemProjection;
  onOpenItem?: (item: WorkItem, target: WorkItemNavigationTarget) => void;
  onResolveAttention?: (attentionId: string) => Promise<void>;
  onDeleteAttention?: (attentionId: string) => Promise<void>;
}) {
  const [resolvingItemId, setResolvingItemId] = useState<string>();
  const [deletingItemId, setDeletingItemId] = useState<string>();
  const [resolveErrors, setResolveErrors] = useState<Record<string, string>>({});
  const busyItemRef = useRef<string>();
  const sections = [
    { id: 'attention', label: '待我处理', items: projection.attention, icon: CircleAlert },
    { id: 'running', label: '后台运行', items: projection.running, icon: LoaderCircle },
    { id: 'continue', label: '继续工作', items: projection.continueWork, icon: MessageSquareMore },
  ] as const;
  const visibleSections = sections.filter(section => section.items.length > 0);

  const resolveItemAttention = async (item: WorkItem) => {
    if (!onResolveAttention || busyItemRef.current) return;
    const unresolved = item.attention.filter(attention => (
      attention.status !== 'resolved' && ACKNOWLEDGEABLE_ATTENTION_KINDS.has(attention.kind)
    ));
    if (unresolved.length === 0) return;
    busyItemRef.current = item.id;
    setResolvingItemId(item.id);
    setResolveErrors(current => ({ ...current, [item.id]: '' }));
    try {
      await Promise.all(unresolved.map(attention => onResolveAttention(attention.id)));
    } catch (reason) {
      setResolveErrors(current => ({
        ...current,
        [item.id]: reason instanceof Error ? reason.message : '标记失败，请重试',
      }));
    } finally {
      busyItemRef.current = undefined;
      setResolvingItemId(undefined);
    }
  };

  const deleteItemAttention = async (item: WorkItem) => {
    if (!onDeleteAttention || busyItemRef.current || item.attention.length === 0) return;
    busyItemRef.current = item.id;
    setDeletingItemId(item.id);
    setResolveErrors(current => ({ ...current, [item.id]: '' }));
    try {
      const results = await Promise.allSettled(
        item.attention.map(attention => onDeleteAttention(attention.id)),
      );
      const failed = results.find((result): result is PromiseRejectedResult => result.status === 'rejected');
      if (failed) throw failed.reason;
    } catch (reason) {
      setResolveErrors(current => ({
        ...current,
        [item.id]: reason instanceof Error ? reason.message : '删除失败，请重试',
      }));
    } finally {
      busyItemRef.current = undefined;
      setDeletingItemId(undefined);
    }
  };

  if (visibleSections.length === 0) {
    return (
      <div className="work-inbox-empty">
        <MessageSquareMore size={20} />
        <strong>目前没有协作事项</strong>
        <span>后台协作者的进度、审阅和产出会集中出现在这里。</span>
      </div>
    );
  }

  return (
    <div className="work-inbox" aria-label="协作收件箱">
      {visibleSections.map(section => (
        <section className={`work-inbox-section is-${section.id}`} key={section.id}>
          <header>
            <section.icon size={15} className={section.id === 'running' ? 'animate-spin' : ''} aria-hidden="true" />
            <h3>{section.label}</h3>
            <span>{section.items.length}</span>
          </header>
          <div className="work-inbox-list">
            {section.items.map(item => {
              const view = presentRuntimeTask(item.task);
              const hasAcknowledgeableAttention = item.attention.some(attention => (
                attention.status !== 'resolved' && ACKNOWLEDGEABLE_ATTENTION_KINDS.has(attention.kind)
              ));
              const isResolving = resolvingItemId === item.id;
              const isDeleting = deletingItemId === item.id;
              const primaryTarget: WorkItemNavigationTarget = (
                (item.task.reviewStatus === 'pending' || item.task.reviewStatus === 'merge_required')
                && view.changedFiles[0]
              )
                ? { kind: 'diff', path: view.changedFiles[0].path }
                : view.documentId
                  ? { kind: 'document', documentId: view.documentId }
                  : { kind: 'card' };
              return (
                <article className="work-inbox-item" key={item.id}>
                  <button type="button" className="work-inbox-item-main" onClick={() => onOpenItem?.(item, primaryTarget)}>
                    <span className={`work-inbox-status is-${view.statusIcon}`} aria-hidden="true" />
                    <span className="work-inbox-copy">
                      <strong>{view.documentTitle}</strong>
                      <small>{[
                        item.tasks.length > 1 ? `${item.tasks.length} 位协作者` : undefined,
                        view.roleLabel,
                        view.displayName,
                        item.sessionTitle,
                        view.statusLabel,
                      ].filter(Boolean).join(' · ')}</small>
                    </span>
                  </button>
                  <div className="work-inbox-item-actions">
                    {view.documentId && (
                      <button
                        type="button"
                        onClick={() => onOpenItem?.(item, { kind: 'document', documentId: view.documentId! })}
                        title="打开产出文档"
                        aria-label={`打开文档：${view.documentTitle}`}
                      >
                        <FileText size={14} />
                      </button>
                    )}
                    {view.changedFiles.slice(0, 3).map(file => (
                      <button
                        type="button"
                        key={file.path}
                        onClick={() => onOpenItem?.(item, { kind: 'diff', path: file.path })}
                        title={`审阅 ${file.path}`}
                        aria-label={`审阅文件：${file.path}`}
                      >
                        <GitCompare size={14} />
                      </button>
                    ))}
                    {section.id === 'attention' && hasAcknowledgeableAttention && onResolveAttention && (
                      <button
                        type="button"
                        disabled={Boolean(resolvingItemId)}
                        onClick={() => void resolveItemAttention(item)}
                        title={isResolving ? '正在标记为已处理' : '标为已处理'}
                        aria-label={isResolving
                          ? `正在标记为已处理：${view.documentTitle}`
                          : `标为已处理：${view.documentTitle}`}
                      >
                        {isResolving
                          ? <LoaderCircle size={14} className="animate-spin" />
                          : <Check size={14} />}
                      </button>
                    )}
                    {item.attention.length > 0 && onDeleteAttention && (
                      <button
                        type="button"
                        className="is-delete"
                        disabled={Boolean(resolvingItemId || deletingItemId)}
                        onClick={() => void deleteItemAttention(item)}
                        title={isDeleting ? '正在删除收件箱记录' : '删除收件箱记录（不会删除文件）'}
                        aria-label={isDeleting
                          ? `正在删除收件箱记录：${view.documentTitle}`
                          : `删除收件箱记录：${view.documentTitle}`}
                      >
                        {isDeleting
                          ? <LoaderCircle size={14} className="animate-spin" />
                          : <Trash2 size={14} />}
                      </button>
                    )}
                  </div>
                  {resolveErrors[item.id] && (
                    <span className="work-inbox-item-error" role="alert">{resolveErrors[item.id]}</span>
                  )}
                </article>
              );
            })}
          </div>
        </section>
      ))}
    </div>
  );
}
