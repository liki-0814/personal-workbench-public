import type { TodoItem } from '../types';

export function isToday(dateStr: string) {
  if (!dateStr) return false;
  const d = new Date(dateStr);
  const today = new Date();
  return (
    d.getFullYear() === today.getFullYear() &&
    d.getMonth() === today.getMonth() &&
    d.getDate() === today.getDate()
  );
}

export function isThisWeek(dateStr: string) {
  if (!dateStr) return false;
  const d = new Date(dateStr);
  const today = new Date();
  const dayOfWeek = today.getDay();
  const diff = dayOfWeek === 0 ? 6 : dayOfWeek - 1;
  const startOfWeek = new Date(today);
  startOfWeek.setDate(today.getDate() - diff);
  startOfWeek.setHours(0, 0, 0, 0);
  const endOfWeek = new Date(startOfWeek);
  endOfWeek.setDate(startOfWeek.getDate() + 7);
  return d >= startOfWeek && d < endOfWeek;
}

/** 计算任务排序分数：优先级 + 截止日期加权 */
export function getTodoSortScore(todo: TodoItem): number {
  const priorityScore = { high: 1000, medium: 500, low: 0 }[todo.priority || 'medium'] || 0;
  let timeScore = 0;
  if (todo.dueDate) {
    const due = new Date(todo.dueDate);
    const now = new Date();
    now.setHours(0, 0, 0, 0);
    const diffDays = Math.ceil((due.getTime() - now.getTime()) / (1000 * 60 * 60 * 24));
    if (diffDays <= 0) timeScore = 500;
    else if (diffDays <= 1) timeScore = 400;
    else if (diffDays <= 2) timeScore = 300;
    else if (diffDays <= 3) timeScore = 200;
    else if (diffDays <= 7) timeScore = 100;
    else timeScore = 50;
  }
  return priorityScore + timeScore;
}

export function autoSortTodos(todos: TodoItem[]): TodoItem[] {
  return [...todos].sort((a, b) => {
    const scoreA = getTodoSortScore(a);
    const scoreB = getTodoSortScore(b);
    if (scoreB !== scoreA) return scoreB - scoreA;
    return (a.order ?? 0) - (b.order ?? 0);
  });
}

/** 格式化截止日期显示 */
export function formatDueDisplay(dueDate: string): string {
  if (!dueDate) return '';
  if (isToday(dueDate)) return '今天';
  const d = new Date(dueDate);
  const today = new Date();
  const tomorrow = new Date(today);
  tomorrow.setDate(today.getDate() + 1);
  if (d.getFullYear() === tomorrow.getFullYear() && d.getMonth() === tomorrow.getMonth() && d.getDate() === tomorrow.getDate()) {
    return '明天';
  }
  const dayAfter = new Date(today);
  dayAfter.setDate(today.getDate() + 2);
  if (d.getFullYear() === dayAfter.getFullYear() && d.getMonth() === dayAfter.getMonth() && d.getDate() === dayAfter.getDate()) {
    return '后天';
  }
  return `${d.getMonth() + 1}/${d.getDate()}`;
}
