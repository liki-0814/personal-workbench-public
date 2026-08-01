import { useState, useEffect, useMemo, useCallback } from 'react';
import { ChevronDown, ChevronRight, Target } from 'lucide-react';
import { AnimatePresence, motion } from 'framer-motion';
import type { TodoItem, TodoType, Goal } from '@/types';
import { autoSortTodos, isToday } from '@/domain/todo/utils/todoHelpers';
import { classifyTodo } from '@/domain/todo/utils/grouping';
import { groupTodosByObjective, matchesTaskFilter, type TaskFilter } from '@/domain/todo/utils/objectives';
import TaskHero from '../components/TaskHero';
import TaskRow from '../components/TaskRow';
import TaskDrawer from '../components/TaskDrawer';
import GoalForm from '../components/GoalForm';
import { useModalDialog } from '@/shell';

interface Props {
  todos: TodoItem[];
  onAdd: (title: string, type: TodoType, notes?: string, priority?: TodoItem['priority'], dueDate?: string, subTasks?: TodoItem['subTasks'], goalId?: string, keyResultId?: string) => void;
  onCycleStatus: (id: string) => void;
  onRemove: (id: string) => void;
  onUpdate: (id: string, updates: Partial<Omit<TodoItem, 'id'>>) => void;
  requestedTodoId?: string | null;
  onRequestedTodoHandled?: () => void;
  // Goals
  goals?: Goal[];
  onAddGoal?: (title: string, description?: string, emoji?: string, deadline?: string, periodStart?: string, periodEnd?: string, keyResults?: Goal['keyResults']) => void;
  onUpdateGoal?: (id: string, patch: Partial<Omit<Goal, 'id'>>) => void;
  onRemoveGoal?: (id: string) => void;
  // Pomodoro — 仅供行级"启动专注"按钮使用
  pomodoroIsRunning: boolean;
  pomodoroTodayMinutes: number;
  boundTodoId: string | null;
  onBindTodo: (todoId: string | null) => void;
  onStartPomodoro: (todo: TodoItem) => void;
  onStartAi?: (todo: TodoItem) => void;
}

export default function TodoTab({
  todos,
  onAdd,
  onCycleStatus,
  onRemove,
  onUpdate,
  requestedTodoId,
  onRequestedTodoHandled,
  goals = [],
  onAddGoal,
  onUpdateGoal,
  onRemoveGoal,
  pomodoroIsRunning,
  pomodoroTodayMinutes,
  boundTodoId,
  onBindTodo,
  onStartPomodoro,
  onStartAi,
}: Props) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [goalFormOpen, setGoalFormOpen] = useState(false);
  const [aiFocusRequest, setAiFocusRequest] = useState(0);
  const [taskFilter, setTaskFilter] = useState<TaskFilter>('in_progress');
  const [collapsedGoalIds, setCollapsedGoalIds] = useState<Set<string>>(() => new Set());
  const [collapsedKeyResultIds, setCollapsedKeyResultIds] = useState<Set<string>>(() => new Set());

  // 排序好的全集
  const sortedTodos = useMemo(() => autoSortTodos(todos), [todos]);

  const visibleTodos = useMemo(
    () => sortedTodos.filter(todo => matchesTaskFilter(todo, taskFilter)),
    [sortedTodos, taskFilter],
  );
  const objectiveGroups = useMemo(
    () => taskFilter === 'in_progress' ? [] : groupTodosByObjective(sortedTodos, goals, taskFilter),
    [goals, sortedTodos, taskFilter],
  );
  const openCount = useMemo(
    () => todos.filter(todo => matchesTaskFilter(todo, 'in_progress')).length,
    [todos],
  );
  const completedCount = todos.length - openCount;

  // Hero 统计
  const heroStats = useMemo(() => {
    const today = todos.filter(t => classifyTodo(t) === 'today' || (t.completed && t.completedAt && isToday(t.completedAt)));
    const todayDone = today.filter(t => t.completed).length;
    const todayTotal = today.length;
    const week = todos.filter(t => {
      const g = classifyTodo(t);
      return g === 'today' || g === 'thisweek' || g === 'overdue';
    }).length;
    return { todayDone, todayTotal, week };
  }, [todos]);

  // 默认选中第一项的逻辑去掉 —— 抽屉模式下，没主动点击就不弹出
  // 选中已被删除的 → 关掉抽屉
  useEffect(() => {
    if (selectedId && !todos.some(t => t.id === selectedId)) {
      setSelectedId(null);
    }
  }, [todos, selectedId]);

  useEffect(() => {
    if (!requestedTodoId) return;
    if (todos.some(todo => todo.id === requestedTodoId)) setSelectedId(requestedTodoId);
    onRequestedTodoHandled?.();
  }, [onRequestedTodoHandled, requestedTodoId, todos]);

  const selectedTodo = selectedId ? todos.find(t => t.id === selectedId) ?? null : null;

  const handleStartPomodoro = useCallback((todoId: string) => {
    const todo = todos.find(t => t.id === todoId);
    if (!todo) return;
    if (boundTodoId !== todoId) onBindTodo(todoId);
    if (!pomodoroIsRunning) onStartPomodoro(todo);
  }, [todos, boundTodoId, onBindTodo, pomodoroIsRunning, onStartPomodoro]);

  const openGoalManagement = () => {
    setGoalFormOpen(true);
  };

  const closeGoalManagement = useCallback(() => {
    setGoalFormOpen(false);
  }, []);
  const goalModalRef = useModalDialog({ open: goalFormOpen, onClose: closeGoalManagement });

  const renderTaskRow = (todo: TodoItem) => (
    <TaskRow
      key={todo.id}
      todo={todo}
      active={selectedId === todo.id}
      pomodoroIsRunning={pomodoroIsRunning}
      isPomodoroBound={boundTodoId === todo.id}
      goals={goals}
      onSelect={() => setSelectedId(todo.id)}
      onCycleStatus={() => onCycleStatus(todo.id)}
      onProgressChange={(progress) => onUpdate(todo.id, { progress })}
      onRemove={() => onRemove(todo.id)}
      onStartPomodoro={() => handleStartPomodoro(todo.id)}
    />
  );

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Hero + Goals */}
      <TaskHero
        todayDoneCount={heroStats.todayDone}
        todayTotalCount={heroStats.todayTotal}
        weekTotalCount={heroStats.week}
        pomodoroTodayMinutes={pomodoroTodayMinutes}
        onAdd={(title, type, notes, priority, dueDate, subTasks, goalId, keyResultId) =>
          onAdd(title, type, notes, priority, dueDate, subTasks, goalId, keyResultId)
        }
        goals={goals}
        onCreateGoal={(data) => {
          onAddGoal?.(data.title, data.description, undefined, undefined, undefined, undefined, data.keyResults);
        }}
        onAppendKeyResults={(goalId, keyResults) => {
          const goal = goals.find(item => item.id === goalId);
          if (!goal) return;
          onUpdateGoal?.(goalId, { keyResults: [...(goal.keyResults ?? []), ...keyResults] });
        }}
        onManageGoals={openGoalManagement}
        aiFocusRequest={aiFocusRequest}
      />

      <div className="mb-4 flex items-center gap-6 border-b border-[#E6E8EC] dark:border-[#252A32]" aria-label="任务状态筛选">
        {([
          ['all', '全部', todos.length],
          ['in_progress', '进行中', openCount],
          ['completed', '已完成', completedCount],
        ] as const).map(([value, label]) => (
          <button
            key={value}
            type="button"
            onClick={() => setTaskFilter(value)}
            className={`relative px-0.5 pb-2.5 pt-1 text-[12px] font-medium transition-colors after:absolute after:inset-x-0 after:bottom-[-1px] after:h-0.5 after:origin-center after:rounded-full after:transition-transform focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/45 focus-visible:ring-offset-2 dark:focus-visible:ring-offset-[#111318] ${
              taskFilter === value
                ? 'text-[#3559D6] after:scale-x-100 after:bg-[#5B6FEA] dark:text-[#9DB0FF] dark:after:bg-[#8EA4F8]'
                : 'text-[#69717D] after:scale-x-0 hover:text-[#20242B] dark:text-[#98A1AD] dark:hover:text-[#E9ECF1]'
            }`}
          >
            {label}
            <span className="ml-1 font-mono text-[10px] tabular-nums opacity-65">
              {value === 'all' ? todos.length : value === 'in_progress' ? openCount : completedCount}
            </span>
          </button>
        ))}
      </div>

      <div className="space-y-3 pr-1">
        {visibleTodos.length === 0 ? (
          <div className="flex min-h-40 items-center justify-center rounded-[14px] border border-[#D9DDE4] bg-white text-[12px] text-[#69717D] dark:border-[#2A3039] dark:bg-[#14171C] dark:text-[#98A1AD]">
            当前没有符合筛选条件的任务
          </div>
        ) : taskFilter === 'in_progress' ? (
          <ul className="overflow-hidden rounded-[14px] border border-[#D9DDE4] bg-white px-3 py-2 dark:border-[#2A3039] dark:bg-[#14171C]" aria-label="进行中的任务">
            {visibleTodos.map(renderTaskRow)}
          </ul>
        ) : objectiveGroups.map(group => {
          const title = group.goal?.title ?? '未归属任务';
          const goalCollapseId = group.goal?.id ?? 'unassigned';
          const goalCollapsed = collapsedGoalIds.has(goalCollapseId);
          return (
            <section key={goalCollapseId} className="overflow-hidden rounded-[14px] border border-[#D9DDE4] bg-white dark:border-[#2A3039] dark:bg-[#14171C]">
              <header className="flex items-center gap-3 border-b border-[#E6E8EC] px-4 py-3 dark:border-[#252A32]">
                <button
                  type="button"
                  onClick={() => setCollapsedGoalIds(current => {
                    const next = new Set(current);
                    if (next.has(goalCollapseId)) next.delete(goalCollapseId);
                    else next.add(goalCollapseId);
                    return next;
                  })}
                  className="-ml-1 rounded-[5px] p-1 text-[#8A919C] hover:bg-[#EEF0F3] hover:text-[#3559D6] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/45 dark:text-[#7E8794] dark:hover:bg-[#242A32] dark:hover:text-[#9DB0FF]"
                  aria-label={`${goalCollapsed ? '展开' : '收起'} ${title}`}
                >
                  {goalCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                </button>
                <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[9px] bg-[#EEF1FC] text-[#3559D6] dark:bg-[#202639] dark:text-[#9DB0FF]">
                  {group.goal?.emoji ?? <Target size={14} />}
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-[10px] font-semibold uppercase tracking-[0.12em] text-[#8A919C] dark:text-[#7E8794]">{group.goal ? 'O' : '收件区'}</span>
                    <h3 className="truncate text-[13px] font-semibold text-[#20242B] dark:text-[#E9ECF1]">{title}</h3>
                  </div>
                  {group.goal?.description && <p className="mt-0.5 truncate text-[11px] text-[#69717D] dark:text-[#98A1AD]">{group.goal.description}</p>}
                </div>
                <div className="text-right">
                  {(group.goal?.periodStart || group.goal?.periodEnd) && (
                    <div className="font-mono text-[9px] text-[#8A919C] dark:text-[#7E8794]">
                      {group.goal.periodStart ?? '—'} → {group.goal.periodEnd ?? '—'}
                    </div>
                  )}
                  <span className="font-mono text-[10px] tabular-nums text-[#69717D] dark:text-[#98A1AD]">{group.completed}/{group.total} 任务</span>
                </div>
              </header>
              {!goalCollapsed && (
                <div className="space-y-2 px-3 py-2">
                  {group.keyResults.map(result => {
                    const keyResultCollapseId = result.keyResult?.id ?? `${goalCollapseId}-without-kr`;
                    const keyResultCollapsed = collapsedKeyResultIds.has(keyResultCollapseId);
                    return (
                      <section key={keyResultCollapseId} className="overflow-hidden rounded-[10px] border border-[#E6E8EC] dark:border-[#252A32]">
                        <button
                          type="button"
                          onClick={() => setCollapsedKeyResultIds(current => {
                            const next = new Set(current);
                            if (next.has(keyResultCollapseId)) next.delete(keyResultCollapseId);
                            else next.add(keyResultCollapseId);
                            return next;
                          })}
                          className="flex w-full items-start gap-2 bg-[#FAFAFB] px-3 py-2.5 text-left transition-colors hover:bg-[#F4F5F7] dark:bg-[#181B21] dark:hover:bg-[#1D2128]"
                        >
                          <span className="mt-0.5 shrink-0 text-[#8A919C] dark:text-[#7E8794]">
                            {keyResultCollapsed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
                          </span>
                          <span className="mt-0.5 shrink-0 font-mono text-[10px] font-semibold text-[#5B6FEA] dark:text-[#9DB0FF]">
                            {result.keyResult?.code ?? (group.goal ? '未归属 KR' : '未归属')}
                          </span>
                          <span className="min-w-0 flex-1 text-[11px] leading-5 text-[#3F4650] dark:text-[#C6CCD5]">
                            {result.keyResult?.title ?? (group.goal ? '这些任务尚未归入 KR' : '这些任务尚未归入 O')}
                          </span>
                          <span className="shrink-0 font-mono text-[9px] tabular-nums text-[#8A919C] dark:text-[#7E8794]">{result.completed}/{result.total}</span>
                        </button>
                        {!keyResultCollapsed && (
                          <ul className="px-2 py-1.5" aria-label={`${result.keyResult?.title ?? '未归属'}任务`}>
                            {result.todos.map(renderTaskRow)}
                          </ul>
                        )}
                      </section>
                    );
                  })}
                </div>
              )}
            </section>
          );
        })}
      </div>

      {/* 抽屉 */}
      <TaskDrawer
        todo={selectedTodo}
        onClose={() => {
          const closingId = selectedId;
          setSelectedId(null);
          requestAnimationFrame(() => {
            if (closingId) document.querySelector<HTMLButtonElement>(`[data-task-id="${CSS.escape(closingId)}"]`)?.focus();
          });
        }}
        onUpdate={onUpdate}
        goals={goals}
        onStartAi={onStartAi}
      />

      <AnimatePresence>
        {goalFormOpen && (
          <motion.div
            className="fixed inset-0 z-[80] flex items-center justify-center overflow-y-auto bg-[#11151B]/35 p-4 backdrop-blur-[2px] dark:bg-black/55"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.16, ease: 'easeOut' }}
            onMouseDown={event => {
              if (event.target === event.currentTarget) closeGoalManagement();
            }}
          >
            <motion.div
              ref={goalModalRef}
              role="dialog"
              aria-modal="true"
              aria-labelledby="goal-management-title"
              tabIndex={-1}
              className="max-h-[min(760px,calc(100vh-2rem))] w-full max-w-[760px] overflow-y-auto rounded-[16px] shadow-[0_28px_80px_rgba(17,22,56,0.22)] outline-none dark:shadow-[0_32px_90px_rgba(0,0,0,0.55)]"
              initial={{ opacity: 0, y: 10, scale: 0.985 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 6, scale: 0.99 }}
              transition={{ duration: 0.2, ease: [0.23, 1, 0.32, 1] }}
            >
              <GoalForm
                goals={goals}
                onSave={(data) => {
                  onAddGoal?.(data.title, data.description, undefined, data.deadline, data.periodStart, data.periodEnd, data.keyResults);
                }}
                onUpdate={(id, data) => onUpdateGoal?.(id, data)}
                onDelete={(id) => onRemoveGoal?.(id)}
                onCancel={closeGoalManagement}
                onAiCreate={() => {
                  setGoalFormOpen(false);
                  setAiFocusRequest(request => request + 1);
                }}
              />
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
