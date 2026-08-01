import { useState, useCallback, useMemo } from 'react';
import { load, save, KEYS, useStorageSync } from '@/core/storage';
import { generateId } from '@/core/utils/id';
import type { PomodoroRecord } from '../types';

export function usePomodoroRecords() {
  const [records, setRecords] = useState<PomodoroRecord[]>(() =>
    load(KEYS.POMODORO_RECORDS, [])
  );

  useStorageSync(() => {
    setRecords(load(KEYS.POMODORO_RECORDS, []));
  });

  const addRecord = useCallback((record: Omit<PomodoroRecord, 'id'>) => {
    const newRecord: PomodoroRecord = { ...record, id: generateId() };
    setRecords(prev => {
      const next = [...prev, newRecord];
      save(KEYS.POMODORO_RECORDS, next);
      return next;
    });
    return newRecord;
  }, []);

  const removeRecord = useCallback((id: string) => {
    setRecords(prev => {
      const next = prev.filter(r => r.id !== id);
      save(KEYS.POMODORO_RECORDS, next);
      return next;
    });
  }, []);

  // 按任务聚合的总专注时长（分钟）
  const taskStats = useMemo(() => {
    const map = new Map<string, { todoId: string; todoTitle: string; totalSeconds: number; count: number }>();
    for (const r of records) {
      if (r.mode !== 'work' || !r.todoId) continue;
      const existing = map.get(r.todoId);
      if (existing) {
        existing.totalSeconds += r.duration;
        existing.count += 1;
      } else {
        map.set(r.todoId, {
          todoId: r.todoId,
          todoTitle: r.todoTitle || '未知任务',
          totalSeconds: r.duration,
          count: 1,
        });
      }
    }
    return Array.from(map.values())
      .map(s => ({
        ...s,
        totalMinutes: Math.round(s.totalSeconds / 60 * 10) / 10,
      }))
      .sort((a, b) => b.totalSeconds - a.totalSeconds);
  }, [records]);

  // 按日期聚合的专注时长
  const dailyStats = useMemo(() => {
    const map = new Map<string, number>();
    for (const r of records) {
      if (r.mode !== 'work') continue;
      const date = r.startTime.slice(0, 8); // yyyyMMdd
      map.set(date, (map.get(date) || 0) + r.duration);
    }
    return Array.from(map.entries())
      .map(([date, seconds]) => ({
        date: `${date.slice(0, 4)}-${date.slice(4, 6)}-${date.slice(6, 8)}`,
        minutes: Math.round(seconds / 60 * 10) / 10,
      }))
      .sort((a, b) => a.date.localeCompare(b.date))
      .slice(-30); // 最近30天
  }, [records]);

  return {
    records,
    addRecord,
    removeRecord,
    taskStats,
    dailyStats,
  };
}
