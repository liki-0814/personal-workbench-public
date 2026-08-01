import type { TodoItem } from '../types';
import { isToday, isThisWeek } from './todoHelpers';

export type TodoGroup = 'overdue' | 'today' | 'thisweek' | 'later' | 'no_date' | 'completed';

export interface TodoGroupMeta {
  key: TodoGroup;
  title: string;
  accent: string;        // CSS color for section title accent
  defaultOpen: boolean;
}

export const GROUP_META: Record<TodoGroup, TodoGroupMeta> = {
  overdue:   { key: 'overdue',   title: '逾期',   accent: '#ef4444', defaultOpen: true },
  today:     { key: 'today',     title: '今日',   accent: '#f59e0b', defaultOpen: true },
  thisweek:  { key: 'thisweek',  title: '本周',   accent: '#22d3ee', defaultOpen: true },
  later:     { key: 'later',     title: '之后',   accent: '#6366f1', defaultOpen: false },
  no_date:   { key: 'no_date',   title: '无日期', accent: '#a78bfa', defaultOpen: false },
  completed: { key: 'completed', title: '已完成', accent: '#10b981', defaultOpen: false },
};

const GROUP_ORDER: TodoGroup[] = ['overdue', 'today', 'thisweek', 'later', 'no_date', 'completed'];

function isPast(dateStr: string): boolean {
  if (!dateStr) return false;
  const d = new Date(dateStr);
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  return d < today;
}

export function classifyTodo(todo: TodoItem): TodoGroup {
  if (todo.completed) return 'completed';
  const due = todo.dueDate;
  if (due) {
    if (isPast(due)) return 'overdue';
    if (isToday(due)) return 'today';
    if (isThisWeek(due)) return 'thisweek';
    return 'later';
  }
  // 没截止日期，但 type=today 也归到今日
  if (todo.type === 'today') return 'today';
  if (todo.type === 'week') return 'thisweek';
  return 'no_date';
}

export interface GroupedTodos {
  meta: TodoGroupMeta;
  items: TodoItem[];
}

export function groupTodos(todos: TodoItem[]): GroupedTodos[] {
  const buckets: Record<TodoGroup, TodoItem[]> = {
    overdue: [], today: [], thisweek: [], later: [], no_date: [], completed: [],
  };
  todos.forEach(t => buckets[classifyTodo(t)].push(t));
  return GROUP_ORDER
    .map(key => ({ meta: GROUP_META[key], items: buckets[key] }))
    .filter(g => g.items.length > 0);
}

/** 友好相对日期：逾期 N 天 / 今天 / 明天 / 周三 / 5 月 12 日 */
export function formatRelativeDate(dateStr: string): { label: string; tone: 'overdue' | 'today' | 'soon' | 'later' } {
  if (!dateStr) return { label: '', tone: 'later' };
  const d = new Date(dateStr);
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const tomorrow = new Date(today);
  tomorrow.setDate(today.getDate() + 1);
  const dayMs = 86400000;
  const diffDays = Math.round((d.getTime() - today.getTime()) / dayMs);

  if (diffDays < 0) return { label: `逾期 ${-diffDays} 天`, tone: 'overdue' };
  if (diffDays === 0) return { label: '今天', tone: 'today' };
  if (diffDays === 1) return { label: '明天', tone: 'soon' };
  if (diffDays === 2) return { label: '后天', tone: 'soon' };
  if (diffDays < 7) {
    const weekday = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'][d.getDay()];
    return { label: weekday, tone: 'soon' };
  }
  return { label: `${d.getMonth() + 1}月${d.getDate()}日`, tone: 'later' };
}
