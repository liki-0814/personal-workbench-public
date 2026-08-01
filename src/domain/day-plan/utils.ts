import type { DayPlanItem } from './types';

export const DAY_START_MINUTE = 8 * 60;
export const DAY_END_MINUTE = 24 * 60;
export const MIN_DURATION = 30;
export const MAX_DURATION = 24 * 60;

export function localDateKey(date = new Date()): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}

export function minuteOfDay(date = new Date()): number {
  return date.getHours() * 60 + date.getMinutes();
}

export function snapMinute(value: number): number {
  return Math.round(value / 5) * 5;
}

export function clampDuration(value: number, startMinute = DAY_START_MINUTE): number {
  const start = Math.max(DAY_START_MINUTE, Math.min(DAY_END_MINUTE - MIN_DURATION, snapMinute(startMinute)));
  const maxDuration = Math.min(MAX_DURATION, DAY_END_MINUTE - start);
  const snapped = Math.round(value / MIN_DURATION) * MIN_DURATION;
  return Math.max(MIN_DURATION, Math.min(maxDuration, snapped));
}

export function clampStart(value: number, duration = 30): number {
  const maxStart = Math.max(DAY_START_MINUTE, DAY_END_MINUTE - Math.min(MAX_DURATION, duration));
  return Math.min(maxStart, Math.max(DAY_START_MINUTE, snapMinute(value)));
}

export function formatMinute(value: number): string {
  const minute = Math.max(0, Math.min(24 * 60, value));
  return `${String(Math.floor(minute / 60)).padStart(2, '0')}:${String(minute % 60).padStart(2, '0')}`;
}

export function parseTime(value: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(value.trim());
  if (!match) return null;
  const hours = Number(match[1]);
  const minutes = Number(match[2]);
  if (hours > 23 || minutes > 59) return null;
  return hours * 60 + minutes;
}

export function plansForToday(items: DayPlanItem[], date = new Date()): DayPlanItem[] {
  const today = localDateKey(date);
  return items.filter(item => item.date === today).sort((a, b) => a.startMinute - b.startMinute);
}
