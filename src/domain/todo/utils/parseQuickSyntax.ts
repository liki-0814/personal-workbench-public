import type { TodoItem } from '../types';
import { formatDate } from '@/core/utils/date';

export interface ParsedQuickSyntax {
  title: string;
  priority?: TodoItem['priority'];
  dueDate?: string;
}

export function parseQuickSyntax(input: string): ParsedQuickSyntax {
  let title = input.trim();
  let priority: TodoItem['priority'] | undefined;
  let dueDate: string | undefined;

  const priorityMatch = title.match(/#(高|中|低|h|m|l|high|medium|low)\b/i);
  if (priorityMatch) {
    const p = priorityMatch[1].toLowerCase();
    if (p === '高' || p === 'h' || p === 'high') priority = 'high';
    else if (p === '中' || p === 'm' || p === 'medium') priority = 'medium';
    else if (p === '低' || p === 'l' || p === 'low') priority = 'low';
    title = title.replace(priorityMatch[0], '').trim();
  }

  const dueMatch = title.match(
    /@(今天|明天|后天|大后天|下周|\d{1,2}\/\d{1,2}|\d{4}-\d{2}-\d{2})/
  );
  if (dueMatch) {
    const d = dueMatch[1];
    const today = new Date();
    if (d === '今天') {
      dueDate = formatDate(today);
    } else if (d === '明天') {
      const tmr = new Date(today);
      tmr.setDate(tmr.getDate() + 1);
      dueDate = formatDate(tmr);
    } else if (d === '后天') {
      const dayAfter = new Date(today);
      dayAfter.setDate(dayAfter.getDate() + 2);
      dueDate = formatDate(dayAfter);
    } else if (d === '大后天') {
      const dayAfter3 = new Date(today);
      dayAfter3.setDate(dayAfter3.getDate() + 3);
      dueDate = formatDate(dayAfter3);
    } else if (d === '下周') {
      const nextWeek = new Date(today);
      const dayOfWeek = today.getDay();
      const diff = dayOfWeek === 0 ? 1 : 8 - dayOfWeek;
      nextWeek.setDate(today.getDate() + diff);
      dueDate = formatDate(nextWeek);
    } else if (d.includes('/')) {
      const [month, day] = d.split('/').map(Number);
      const year = today.getFullYear();
      dueDate = `${year}-${String(month).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
    } else if (d.includes('-')) {
      dueDate = d;
    }
    title = title.replace(dueMatch[0], '').trim();
  }

  return { title, priority, dueDate };
}
