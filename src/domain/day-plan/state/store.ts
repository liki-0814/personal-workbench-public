import { useCallback, useState } from 'react';
import { KEYS, load, save, useStorageSync } from '@/core/storage';
import { generateId } from '@/core/utils/id';
import { now } from '@/core/utils/date';
import type { DayPlanItem } from '../types';
import { clampDuration, clampStart, localDateKey, plansForToday } from '../utils';

type LegacyTodo = {
  id: string;
  title: string;
  isIntradayPlan?: boolean;
  scheduledStart?: string;
  scheduledEnd?: string;
  createdAt?: string;
  [key: string]: unknown;
};

function parseLegacyMinute(value?: string): number | null {
  if (!value) return null;
  const match = /^(\d{1,2}):(\d{2})$/.exec(value);
  if (!match) return null;
  return Number(match[1]) * 60 + Number(match[2]);
}

function loadAndMigrateTodayPlans(): DayPlanItem[] {
  const today = localDateKey();
  const stored = load<DayPlanItem[]>(KEYS.DAY_PLANS, []);
  const legacyTodos = load<LegacyTodo[]>(KEYS.TODOS, []);
  const legacyPlans = legacyTodos.filter(todo => todo.isIntradayPlan);
  const cleanedTodos = legacyTodos.filter(todo => !todo.isIntradayPlan).map(todo => {
    const { isIntradayPlan: _intraday, scheduledStart: _start, scheduledEnd: _end, ...clean } = todo;
    return clean;
  });

  if (legacyPlans.length > 0 || legacyTodos.some(todo => todo.scheduledStart || todo.scheduledEnd)) {
    const migrated = legacyPlans.map((todo, index): DayPlanItem => {
      const start = parseLegacyMinute(todo.scheduledStart) ?? clampStart(new Date().getHours() * 60 + new Date().getMinutes() + index * 30);
      const end = parseLegacyMinute(todo.scheduledEnd);
      const timestamp = todo.createdAt || now();
      return {
        id: `day-${todo.id}`,
        date: today,
        title: todo.title,
        startMinute: start,
        durationMinutes: end && end > start ? end - start : 30,
        createdAt: timestamp,
        updatedAt: timestamp,
      };
    });
    const existingIds = new Set(stored.map(item => item.id));
    save(KEYS.TODOS, cleanedTodos);
    save(KEYS.DAY_PLANS, [...stored, ...migrated.filter(item => !existingIds.has(item.id))]);
    return plansForToday([...stored, ...migrated.filter(item => !existingIds.has(item.id))]);
  }

  const todayPlans = plansForToday(stored);
  if (todayPlans.length !== stored.length) save(KEYS.DAY_PLANS, todayPlans);
  return todayPlans;
}

export function useDayPlans() {
  const [plans, setPlans] = useState<DayPlanItem[]>(loadAndMigrateTodayPlans);

  useStorageSync(() => setPlans(plansForToday(load<DayPlanItem[]>(KEYS.DAY_PLANS, []))));

  const changePlans = useCallback((change: (current: DayPlanItem[]) => DayPlanItem[]) => {
    setPlans(current => {
      const next = plansForToday(change(current));
      save(KEYS.DAY_PLANS, next);
      return next;
    });
  }, []);

  const addPlan = useCallback((title: string, startMinute: number, durationMinutes = 30, sourceTodoId?: string) => {
    const timestamp = now();
    const start = clampStart(startMinute);
    const duration = clampDuration(durationMinutes, start);
    const item: DayPlanItem = {
      id: generateId(),
      date: localDateKey(),
      title: title.trim(),
      startMinute: start,
      durationMinutes: duration,
      sourceTodoId,
      createdAt: timestamp,
      updatedAt: timestamp,
    };
    changePlans(current => [...current, item]);
  }, [changePlans]);

  const updatePlan = useCallback((id: string, patch: Partial<Pick<DayPlanItem, 'title' | 'startMinute' | 'durationMinutes'>>) => {
    changePlans(current => current.map(item => {
      if (item.id !== id) return item;
      if (patch.startMinute === undefined && patch.durationMinutes === undefined) {
        return { ...item, ...patch, updatedAt: now() };
      }
      const startMinute = clampStart(patch.startMinute ?? item.startMinute);
      const durationMinutes = clampDuration(patch.durationMinutes ?? item.durationMinutes, startMinute);
      return { ...item, ...patch, startMinute, durationMinutes, updatedAt: now() };
    }));
  }, [changePlans]);

  const removePlan = useCallback((id: string) => {
    const removed = plans.find(item => item.id === id);
    if (!removed) return undefined;
    changePlans(current => current.filter(item => item.id !== id));
    return removed;
  }, [changePlans, plans]);

  const restorePlan = useCallback((item: DayPlanItem) => {
    changePlans(current => current.some(candidate => candidate.id === item.id) ? current : [...current, item]);
  }, [changePlans]);

  return { plans, addPlan, updatePlan, removePlan, restorePlan };
}
