import type { HabitFrequency, HabitRecord } from './types';
import { formatDate } from '@/core/utils/date';

/** Get the day of week (0=Sun..6=Sat) for a YYYY-MM-DD string. */
export function getDayOfWeekForDate(dateStr: string): number {
  const d = new Date(dateStr + 'T00:00:00');
  return d.getDay();
}

/** Check if a given date is a due day for the habit based on its frequency. */
export function isDueOnDate(frequency: HabitFrequency, dateStr: string): boolean {
  switch (frequency.type) {
    case 'daily':
      return true;
    case 'weekdays':
      return frequency.days.includes(getDayOfWeekForDate(dateStr));
    case 'weekly':
      // Weekly habits are always "due" — streak logic checks weekly targets
      return true;
  }
}

/** Check if the habit is due on the given day (defaults to today). */
export function isDueOn(frequency: HabitFrequency, date: Date = new Date()): boolean {
  return isDueOnDate(frequency, formatDate(date));
}

function toDoneSet(records: HabitRecord[]): Set<string> {
  return new Set(records.filter(r => r.done).map(r => r.date));
}

/** The unit a streak counts in — days for daily/weekdays habits, weeks for weekly ones. */
export function streakUnit(frequency: HabitFrequency): 'day' | 'week' {
  return frequency.type === 'weekly' ? 'week' : 'day';
}

/**
 * Current streak for a habit.
 * Daily/weekdays: consecutive due days completed, counting back from today.
 * A due-but-undone today does not break the streak (counting starts yesterday).
 * Weekly: consecutive Mon-Sun weeks meeting the timesPerWeek target,
 * ignoring the still-incomplete current week.
 */
export function computeStreak(
  frequency: HabitFrequency,
  records: HabitRecord[],
  today: Date,
): number {
  const doneSet = toDoneSet(records);

  if (frequency.type === 'weekly') {
    return computeWeeklyStreak(frequency.timesPerWeek, doneSet, today);
  }

  const cursor = new Date(today);
  const todayStr = formatDate(cursor);
  if (isDueOnDate(frequency, todayStr) && !doneSet.has(todayStr)) {
    cursor.setDate(cursor.getDate() - 1);
  }

  let streak = 0;
  for (let i = 0; i < 365; i++) {
    const dateStr = formatDate(cursor);
    if (isDueOnDate(frequency, dateStr)) {
      if (!doneSet.has(dateStr)) break;
      streak++;
    }
    cursor.setDate(cursor.getDate() - 1);
  }
  return streak;
}

/** Done flags for the last 7 days, oldest first (today-6 → today). */
export function computeWeekStatus(records: HabitRecord[], today: Date): boolean[] {
  const doneSet = toDoneSet(records);
  const result: boolean[] = [];
  for (let i = 6; i >= 0; i--) {
    const d = new Date(today);
    d.setDate(d.getDate() - i);
    result.push(doneSet.has(formatDate(d)));
  }
  return result;
}

/**
 * Calculate weekly streak: how many consecutive weeks (Mon-Sun) the habit met
 * its timesPerWeek target. The current, still-incomplete week is skipped.
 */
export function computeWeeklyStreak(
  timesPerWeek: number,
  doneSet: Set<string>,
  today: Date,
): number {
  let streak = 0;

  // Find the start of the current week (Monday)
  const dayOfWeek = today.getDay();
  const diffToMonday = dayOfWeek === 0 ? 6 : dayOfWeek - 1;

  // Start checking from last completed week (skip current incomplete week)
  const weekStart = new Date(today);
  weekStart.setDate(today.getDate() - diffToMonday - 7);

  for (let w = 0; w < 52; w++) {
    let count = 0;
    for (let i = 0; i < 7; i++) {
      const checkDate = new Date(weekStart);
      checkDate.setDate(weekStart.getDate() + i);
      if (doneSet.has(formatDate(checkDate))) {
        count++;
      }
    }

    if (count >= timesPerWeek) {
      streak++;
    } else {
      break;
    }

    // Move to previous week
    weekStart.setDate(weekStart.getDate() - 7);
  }

  return streak;
}
