import { Trash2, Timer, ChevronRight } from 'lucide-react';
import type { TodoItem, Goal } from '../../types';
import { getTodoProgress } from '../../utils/progress';
import TaskMetaChips from './TaskMetaChips';
import TaskStatusCheck from './TaskStatusCheck';

interface Props {
  todo: TodoItem;
  active: boolean;
  pomodoroIsRunning: boolean;
  isPomodoroBound: boolean;
  goals?: Goal[];
  onSelect: () => void;
  onCycleStatus: () => void;
  onProgressChange: (progress: number) => void;
  onRemove: () => void;
  onStartPomodoro: () => void;
}

const PRIORITY_BAR_COLOR: Record<NonNullable<TodoItem['priority']>, string> = {
  high: '#C94C4C',
  medium: '#B58124',
  low: '#5276C8',
};

export default function TaskRow({
  todo,
  active,
  pomodoroIsRunning,
  isPomodoroBound,
  goals,
  onSelect,
  onCycleStatus,
  onProgressChange,
  onRemove,
  onStartPomodoro,
}: Props) {
  const isFocusing = isPomodoroBound && pomodoroIsRunning;
  const progress = getTodoProgress(todo);

  const moveFocus = (direction: -1 | 1 | 'first' | 'last') => {
    const rows = Array.from(document.querySelectorAll<HTMLButtonElement>('[data-task-primary="true"]'));
    const current = rows.findIndex(row => row.dataset.taskId === todo.id);
    const next = direction === 'first'
      ? rows[0]
      : direction === 'last'
        ? rows[rows.length - 1]
        : rows[current + direction];
    next?.focus();
  };

  return (
    <li
      data-id={todo.id}
      className={`group relative flex min-h-[52px] items-center gap-2.5 border-b border-[#E4E6EA] px-2 py-2 transition-colors focus-within:bg-[#F7F8FA] dark:border-[#242A32] dark:focus-within:bg-[#171B21] ${
        active
          ? 'bg-[#EEF0F3] dark:bg-[#1A1E25]'
          : 'hover:bg-[#F7F8FA] dark:hover:bg-[#171B21]'
      } ${todo.completed ? 'opacity-60' : ''}`}
    >
      {/* 左侧 4px 优先级色条 */}
      {todo.priority && !todo.completed && (
        <span
          className="absolute bottom-2 left-0 top-2 w-0.5 rounded-r"
          style={{ background: PRIORITY_BAR_COLOR[todo.priority] }}
        />
      )}

      {/* 3 态 check */}
      <TaskStatusCheck
        status={todo.status}
        completed={todo.completed}
        onClick={(e) => { e.stopPropagation(); onCycleStatus(); }}
      />

      {/* 标题 + 元信息 */}
      <div className="min-w-0 flex-1">
        <button
          type="button"
          data-task-primary="true"
          data-task-id={todo.id}
          aria-current={active ? 'true' : undefined}
          onClick={onSelect}
          onKeyDown={event => {
            if (event.key === 'ArrowDown') { event.preventDefault(); moveFocus(1); }
            else if (event.key === 'ArrowUp') { event.preventDefault(); moveFocus(-1); }
            else if (event.key === 'Home') { event.preventDefault(); moveFocus('first'); }
            else if (event.key === 'End') { event.preventDefault(); moveFocus('last'); }
          }}
          className={`block w-full truncate rounded-[5px] text-left text-[13px] leading-5 text-[#30353D] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 dark:text-[#D9DEE6] ${todo.completed ? 'line-through text-[#8A919C] dark:text-[#707986]' : ''}`}
        >
          {todo.title}
        </button>
        <TaskMetaChips todo={todo} goals={goals} />
        {!todo.completed && (
          <div className="mt-1.5 flex items-center gap-2">
            <input
              type="range"
              min={0}
              max={100}
              step={1}
              value={progress}
              aria-label={`${todo.title} 完成进度`}
              aria-valuetext={`${progress}%`}
              onPointerDown={(event) => event.stopPropagation()}
              onClick={(event) => event.stopPropagation()}
              onChange={(event) => onProgressChange(Number(event.target.value))}
              className="h-1.5 min-w-0 flex-1 cursor-pointer accent-[#5B6FEA] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/45 dark:accent-[#8EA4F8]"
            />
            <span className="w-8 shrink-0 text-right font-mono text-[10px] tabular-nums text-[#69717D] dark:text-[#98A1AD]">
              {progress}%
            </span>
          </div>
        )}
      </div>

      {/* 操作区 */}
      <div className="task-row-actions flex flex-shrink-0 items-center gap-0.5 group-focus-within:opacity-100">
        {!todo.completed && (
          <button
            onClick={(e) => { e.stopPropagation(); onStartPomodoro(); }}
            className={`rounded-[6px] p-1.5 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 ${
              isFocusing
                ? 'bg-[#FBEFEF] text-[#A33D3D] dark:bg-[#2C2023] dark:text-[#F19A9A]'
                : 'text-[#7A818C] opacity-0 hover:bg-[#EEF0F3] hover:text-[#A33D3D] group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 dark:text-[#8D96A3] dark:hover:bg-[#242932] dark:hover:text-[#F19A9A]'
            }`}
            title={isFocusing ? '专注中' : '开始专注'}
          >
            <Timer size={13} />
          </button>
        )}
        <button
          onClick={(e) => { e.stopPropagation(); onRemove(); }}
          className="rounded-[6px] p-1.5 text-[#7A818C] opacity-0 transition-colors hover:bg-[#FBEFEF] hover:text-[#A33D3D] focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 group-hover:opacity-100 dark:text-[#8D96A3] dark:hover:bg-[#2C2023] dark:hover:text-[#F19A9A]"
          title="删除"
        >
          <Trash2 size={13} />
        </button>
        <ChevronRight size={14} aria-hidden="true" className="text-[#A4AAB3] opacity-0 group-hover:opacity-70 group-focus-within:opacity-70 dark:text-[#626B77]" />
      </div>
    </li>
  );
}
