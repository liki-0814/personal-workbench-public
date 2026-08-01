import { useState, useCallback } from 'react';
import type { HabitItem, HabitFrequency } from '../types';
import { load, save, KEYS, useStorageSync } from '@/core/storage';
import { now, formatDate } from '@/core/utils/date';

function generateHabitId(): string {
  return `habit-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
}

/** Get today's date string in YYYY-MM-DD. */
function today(): string {
  return formatDate(new Date());
}

/** Get the day of week (0=Sun..6=Sat) for a YYYY-MM-DD string. */
function getDayOfWeekForDate(dateStr: string): number {
  const d = new Date(dateStr + 'T00:00:00');
  return d.getDay();
}

/** Check if a given date is a due day for the habit based on its frequency. */
function isDueOnDate(frequency: HabitFrequency, dateStr: string): boolean {
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

export function useHabits() {
  const [allHabits, setAllHabits] = useState<HabitItem[]>(() =>
    load(KEYS.HABITS, [])
  );

  useStorageSync(() => {
    setAllHabits(load(KEYS.HABITS, []));
  });

  const persist = (items: HabitItem[]) => {
    setAllHabits(items);
    save(KEYS.HABITS, items);
  };

  const habits = allHabits.filter(h => !h.archived);

  const addHabit = useCallback((title: string, emoji: string, frequency: HabitFrequency) => {
    const item: HabitItem = {
      id: generateHabitId(),
      title,
      emoji,
      frequency,
      records: [],
      createdAt: now(),
      updatedAt: now(),
      archived: false,
    };
    persist([...allHabits, item]);
  }, [allHabits]);

  const removeHabit = useCallback((id: string) => {
    persist(allHabits.filter(h => h.id !== id));
  }, [allHabits]);

  const updateHabit = useCallback((id: string, updates: Partial<Omit<HabitItem, 'id'>>) => {
    persist(allHabits.map(h =>
      h.id === id ? { ...h, ...updates, updatedAt: now() } : h
    ));
  }, [allHabits]);

  const toggleToday = useCallback((id: string) => {
    const todayStr = today();
    persist(allHabits.map(h => {
      if (h.id !== id) return h;
      const existingIdx = h.records.findIndex(r => r.date === todayStr);
      let newRecords: typeof h.records;
      if (existingIdx >= 0) {
        // Flip existing record
        newRecords = h.records.map((r, i) =>
          i === existingIdx ? { ...r, done: !r.done } : r
        );
      } else {
        // Add new record as done
        newRecords = [...h.records, { date: todayStr, done: true }];
      }
      return { ...h, records: newRecords, updatedAt: now() };
    }));
  }, [allHabits]);

  const archiveHabit = useCallback((id: string) => {
    persist(allHabits.map(h =>
      h.id === id ? { ...h, archived: true, updatedAt: now() } : h
    ));
  }, [allHabits]);

  const getStreak = useCallback((id: string): number => {
    const habit = allHabits.find(h => h.id === id);
    if (!habit) return 0;

    const { frequency, records } = habit;
    const doneSet = new Set(records.filter(r => r.done).map(r => r.date));

    if (frequency.type === 'weekly') {
      return getWeeklyStreak(frequency.timesPerWeek, doneSet);
    }

    // For daily and weekdays: count consecutive due days completed
    let streak = 0;
    const d = new Date();

    // Start from today and go backwards
    for (let i = 0; i < 365; i++) {
      const dateStr = formatDate(d);
      const due = isDueOnDate(frequency, dateStr);

      if (due) {
        if (doneSet.has(dateStr)) {
          streak++;
        } else {
          // If today is not done yet, skip it (don't break streak)
          if (i === 0) {
            streak = 0; // Reset — today counts
            // Actually, let's be lenient: if today isn't done yet, start checking from yesterday
            d.setDate(d.getDate() - 1);
            continue;
          }
          break;
        }
      }

      d.setDate(d.getDate() - 1);
    }

    return streak;
  }, [allHabits]);

  const getWeekStatus = useCallback((id: string): boolean[] => {
    const habit = allHabits.find(h => h.id === id);
    if (!habit) return Array(7).fill(false);

    const doneSet = new Set(habit.records.filter(r => r.done).map(r => r.date));
    const result: boolean[] = [];

    // Last 7 days: today-6 → today
    for (let i = 6; i >= 0; i--) {
      const d = new Date();
      d.setDate(d.getDate() - i);
      result.push(doneSet.has(formatDate(d)));
    }

    return result;
  }, [allHabits]);

  const isTodayDue = useCallback((habit: HabitItem): boolean => {
    return isDueOnDate(habit.frequency, today());
  }, []);

  return {
    habits,
    allHabits,
    addHabit,
    removeHabit,
    updateHabit,
    toggleToday,
    archiveHabit,
    getStreak,
    getWeekStatus,
    isTodayDue,
  };
}

/**
 * Calculate weekly streak: how many consecutive weeks the habit met its
 * timesPerWeek target. Weeks are Mon-Sun.
 */
function getWeeklyStreak(timesPerWeek: number, doneSet: Set<string>): number {
  let streak = 0;
  const d = new Date();

  // Find the start of the current week (Monday)
  const dayOfWeek = d.getDay();
  const diffToMonday = dayOfWeek === 0 ? 6 : dayOfWeek - 1;

  // Start checking from last completed week (skip current incomplete week)
  const weekStart = new Date(d);
  weekStart.setDate(d.getDate() - diffToMonday - 7);

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
