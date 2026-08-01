import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { X, Loader2, CheckCircle2, XCircle, Ban, FileText, Trash2 } from 'lucide-react';
import { apiFetch } from '@/core/utils';
import type { BackgroundTaskInfo } from '../../state/backgroundTaskStore';

interface Props {
  id: string;
  anchorRef: RefObject<HTMLElement>;
  tasks: BackgroundTaskInfo[];
  onCancel: (taskId: string) => void;
  onDismiss: (taskId: string) => void;
  onClose: () => void;
}

function elapsed(secs?: number): string {
  if (!secs || secs <= 0) return '0s';
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}m${s > 0 ? `${String(s).padStart(2, '0')}s` : ''}`;
}

const RUNTIME_STATUS_LABEL: Record<string, string> = {
  awaiting_workspace: '等待目录', queued: '准备中', running: '执行中', awaiting_input: '等待判断',
  awaiting_review: '等待验收', merge_conflict: '合并冲突', paused: '已暂停', completed: '已完成', failed: '失败', cancelled: '已取消',
};

function StatusIcon({ status }: { status: BackgroundTaskInfo['status'] }) {
  switch (status) {
    case 'running':
      return <Loader2 size={14} className="precision-status-primary animate-spin" />;
    case 'completed':
      return <CheckCircle2 size={14} className="precision-status-success" />;
    case 'failed':
      return <XCircle size={14} className="precision-status-danger" />;
    case 'cancelled':
      return <Ban size={14} className="precision-muted-icon" />;
  }
}

function TaskRow({ task, onCancel, onViewLog, onDismiss }: {
  task: BackgroundTaskInfo;
  onCancel: () => void;
  onViewLog: () => void;
  onDismiss: () => void;
}) {
  const isRunning = task.status === 'running';
  const isDone = !isRunning;
  const hasLog = !!task.logFile;

  return (
    <div
      className={`precision-task-row px-3 py-2.5 ${isDone ? 'is-done' : ''} ${hasLog ? 'cursor-pointer' : ''}`}
      data-status={task.status}
      onClick={hasLog ? onViewLog : undefined}
    >
      <div className="flex items-center gap-2 min-w-0">
        <StatusIcon status={task.status} />
        <span className="precision-option-title text-xs font-medium truncate">
          {task.toolName}
          {task.runtimeStatus && <span className="precision-message-meta ml-1">· {RUNTIME_STATUS_LABEL[task.runtimeStatus] ?? task.runtimeStatus}</span>}
        </span>
        <span className="precision-message-meta ml-auto text-[10px] tabular-nums shrink-0">
          {elapsed(isRunning ? task.elapsedSecs : task.durationSecs ?? task.elapsedSecs)}
        </span>
      </div>
      <div className="flex items-center gap-2 mt-1 min-w-0">
        <span className="precision-option-description text-[11px] truncate flex-1">
          {task.description}
          {task.status === 'completed' && task.success !== undefined && (
            <span className={task.success ? ' precision-status-success' : ' precision-status-danger'}>
              {' '}{task.success ? '成功' : '失败'}
            </span>
          )}
        </span>
        {isRunning && (
          <button
            type="button"
            onClick={(e) => { e.stopPropagation(); onCancel(); }}
            className="precision-text-action precision-text-danger shrink-0 text-[10px] px-1.5 py-0.5"
          >
            取消
          </button>
        )}
        {hasLog && isDone && (
          <FileText size={12} className="precision-muted-icon shrink-0" />
        )}
        {isDone && (
          <button
            type="button"
            onClick={(e) => { e.stopPropagation(); onDismiss(); }}
            className="precision-sidebar-icon-button precision-danger-button shrink-0 p-0.5"
            title="移除"
            aria-label={`移除后台任务：${task.description}`}
          >
            <Trash2 size={11} />
          </button>
        )}
      </div>
    </div>
  );
}

function LogModal({ task, onClose }: { task: BackgroundTaskInfo; onClose: () => void }) {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const modalRef = useRef<HTMLDivElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();

  useEffect(() => {
    if (!task.logFile) return;
    setLoading(true);
    apiFetch<{ content?: string }>(`/api/fs/read?path=${encodeURIComponent(task.logFile)}`)
      .then(res => {
        if (typeof res === 'string') {
          setContent(res);
        } else if (res && typeof res.content === 'string') {
          setContent(res.content);
        } else {
          setContent(JSON.stringify(res, null, 2));
        }
      })
      .catch(e => setContent(`读取日志失败: ${e}`))
      .finally(() => setLoading(false));
  }, [task.logFile]);

  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    closeButtonRef.current?.focus();
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
        return;
      }
      if (e.key !== 'Tab' || !modalRef.current) return;
      const focusable = Array.from(
        modalRef.current.querySelectorAll<HTMLElement>('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'),
      ).filter(element => !element.hasAttribute('disabled'));
      if (focusable.length === 0) {
        e.preventDefault();
        modalRef.current.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener('keydown', handler);
    return () => {
      document.removeEventListener('keydown', handler);
      previousFocus?.focus();
    };
  }, [onClose]);

  // 尝试解析 JSON 提取 stdout/stderr
  let display = content || '';
  try {
    const parsed = JSON.parse(display);
    const parts: string[] = [];
    if (parsed.stdout) parts.push(parsed.stdout);
    if (parsed.stderr) parts.push(`[STDERR]\n${parsed.stderr}`);
    if (parts.length > 0) display = parts.join('\n\n');
    if (parsed.exit_code !== undefined && parsed.exit_code !== null) {
      display += `\n\n[exit_code: ${parsed.exit_code}]`;
    }
  } catch { /* not JSON, show raw */ }

  return (
    <div
      className="precision-modal-backdrop fixed inset-0 z-[100] flex items-center justify-center"
      onMouseDown={event => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={modalRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className="precision-log-modal w-[90vw] max-w-4xl h-[80vh] flex flex-col overflow-hidden"
      >
        {/* Header */}
        <div className="precision-modal-header flex items-center justify-between px-5 py-3 shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <StatusIcon status={task.status} />
            <span id={titleId} className="precision-option-title text-sm font-medium truncate">
              {task.toolName}
            </span>
            {task.success !== undefined && (
              <span className={`precision-status-badge text-xs px-2 py-0.5 ${task.success ? 'is-success' : 'is-danger'}`}>
                {task.success ? '成功' : '失败'}
              </span>
            )}
            <span className="precision-message-meta text-xs">
              {elapsed(task.elapsedSecs)}
            </span>
          </div>
          <button
            ref={closeButtonRef}
            type="button"
            onClick={onClose}
            className="precision-icon-button p-1.5"
            aria-label="关闭任务日志"
          >
            <X size={16} />
          </button>
        </div>

        {/* Description */}
        <div className="precision-modal-description px-5 py-2 text-xs shrink-0 truncate">
          {task.description}
        </div>

        {/* Log content */}
        <div className="flex-1 overflow-auto p-5">
          {loading ? (
            <div className="flex items-center justify-center h-full">
              <Loader2 size={24} className="precision-muted-icon animate-spin" />
            </div>
          ) : (
            <pre className="precision-log-content text-xs leading-5 font-mono whitespace-pre-wrap break-words">
              {display}
            </pre>
          )}
        </div>

        {/* Footer */}
        {task.logFile && (
          <div className="precision-modal-footer px-5 py-2 text-[11px] shrink-0 font-mono truncate">
            {task.logFile}
          </div>
        )}
      </div>
    </div>
  );
}

export default function BackgroundTaskPanel({ id, anchorRef, tasks, onCancel, onDismiss, onClose }: Props) {
  const panelRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const viewingTaskRef = useRef<BackgroundTaskInfo | null>(null);
  const onCloseRef = useRef(onClose);
  const [panelPosition, setPanelPosition] = useState({ top: 0, left: 0, width: 320, maxHeight: 384 });
  const [viewingTask, setViewingTask] = useState<BackgroundTaskInfo | null>(null);
  viewingTaskRef.current = viewingTask;
  onCloseRef.current = onClose;
  const closeLog = useCallback(() => setViewingTask(null), []);

  const updatePanelPosition = useCallback(() => {
    const anchor = anchorRef.current;
    if (!anchor) return;

    const gutter = 8;
    const gap = 7;
    const rect = anchor.getBoundingClientRect();
    const width = Math.min(320, window.innerWidth - gutter * 2);
    const desiredHeight = Math.min(384, panelRef.current?.scrollHeight || 384);
    const spaceBelow = window.innerHeight - rect.bottom - gutter;
    const spaceAbove = rect.top - gutter;
    const openAbove = spaceBelow < Math.min(desiredHeight, 180) && spaceAbove > spaceBelow;
    const availableHeight = Math.max(128, (openAbove ? spaceAbove : spaceBelow) - gap);
    const maxHeight = Math.min(384, availableHeight);
    const left = Math.min(
      Math.max(gutter, rect.right - width),
      window.innerWidth - gutter - width,
    );
    const top = openAbove
      ? Math.max(gutter, rect.top - gap - Math.min(desiredHeight, maxHeight))
      : rect.bottom + gap;

    setPanelPosition({ top, left, width, maxHeight });
  }, [anchorRef]);

  useLayoutEffect(() => {
    updatePanelPosition();
    window.addEventListener('resize', updatePanelPosition);
    window.addEventListener('scroll', updatePanelPosition, true);
    return () => {
      window.removeEventListener('resize', updatePanelPosition);
      window.removeEventListener('scroll', updatePanelPosition, true);
    };
  }, [tasks.length, updatePanelPosition]);

  useEffect(() => {
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    const panel = panelRef.current;
    const focusFrame = window.requestAnimationFrame(() => panel?.focus());
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      const anchor = anchorRef.current;
      if (!panelRef.current?.contains(target) && !anchor?.contains(target) && !viewingTaskRef.current) {
        onCloseRef.current();
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || viewingTaskRef.current) return;
      event.preventDefault();
      onCloseRef.current();
    };
    document.addEventListener('pointerdown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.cancelAnimationFrame(focusFrame);
      document.removeEventListener('pointerdown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
      if (panel?.contains(document.activeElement)) previousFocusRef.current?.focus();
    };
  }, [anchorRef]);

  const running = tasks.filter(t => t.status === 'running');
  const done = tasks.filter(t => t.status !== 'running');

  return (
    <>
      {createPortal(<>
        <div
          ref={panelRef}
          id={id}
          role="dialog"
          aria-label="AI 任务"
          tabIndex={-1}
          className="precision-floating-popover precision-background-panel"
          style={panelPosition}
        >
        <div className="precision-popover-header sticky top-0 flex items-center justify-between px-3 py-2">
          <span className="precision-option-title text-xs font-medium">
            AI 任务
            {tasks.length > 0 && (
              <span className="precision-message-meta ml-1.5 tabular-nums">
                ({running.length}/{tasks.length})
              </span>
            )}
          </span>
          <button
            type="button"
            onClick={onClose}
            className="precision-icon-button p-1"
            aria-label="关闭后台任务"
          >
            <X size={14} />
          </button>
        </div>

        {tasks.length === 0 ? (
          <div className="precision-empty-copy px-3 py-6 text-center text-xs">
            暂无 AI 任务
          </div>
        ) : (
          <div className="precision-divide-list">
            {running.map(t => (
              <TaskRow key={t.id} task={t} onCancel={() => onCancel(t.id)} onViewLog={() => setViewingTask(t)} onDismiss={() => {}} />
            ))}
            {done.map(t => (
              <TaskRow key={t.id} task={t} onCancel={() => {}} onViewLog={() => setViewingTask(t)} onDismiss={() => onDismiss(t.id)} />
            ))}
          </div>
        )}
        </div>

        {viewingTask && (
          <LogModal task={viewingTask} onClose={closeLog} />
        )}
      </>, document.body)}
    </>
  );
}
