import { useState, useMemo } from 'react';
import { X } from 'lucide-react';
import type { HabitItem } from '../../types';
import { formatDate } from '@/core/utils/date';
import { SelectField } from '@/shell';

interface Props {
  habits: HabitItem[];
  getStreak: (id: string) => number;
  onClose: () => void;
}

export default function HabitHistory({ habits, getStreak, onClose }: Props) {
  const [selectedHabitId, setSelectedHabitId] = useState<string | ''>('');

  const activeHabits = useMemo(() => habits.filter(h => !h.archived), [habits]);

  // Build lookup of dates where habits were done
  const doneSet = useMemo(() => {
    const set = new Set<string>();
    const source = selectedHabitId
      ? habits.filter(h => h.id === selectedHabitId)
      : activeHabits;
    for (const habit of source) {
      for (const rec of habit.records) {
        if (rec.done) set.add(rec.date);
      }
    }
    return set;
  }, [habits, activeHabits, selectedHabitId]);

  // Generate 12 weeks of dates (columns = weeks, rows = Mon-Sun)
  const { weeks, totalDone, longestStreak, currentStreak } = useMemo(() => {
    const today = new Date();
    // Start from the Monday of 12 weeks ago
    const startDate = new Date(today);
    const dayOfWeek = today.getDay(); // 0=Sun
    const mondayOffset = dayOfWeek === 0 ? 6 : dayOfWeek - 1;
    startDate.setDate(today.getDate() - mondayOffset - 11 * 7);

    const wks: { date: Date; dateStr: string; done: boolean }[][] = [];
    let total = 0;
    let longest = 0;
    let tempStreak = 0;

    const cursor = new Date(startDate);
    let weekArr: { date: Date; dateStr: string; done: boolean }[] = [];

    while (cursor <= today || weekArr.length > 0) {
      if (cursor > today && weekArr.length > 0) {
        wks.push(weekArr);
        break;
      }

      const dateStr = formatDate(cursor);
      const done = doneSet.has(dateStr);
      weekArr.push({ date: new Date(cursor), dateStr, done });

      if (done) {
        total++;
        tempStreak++;
        if (tempStreak > longest) longest = tempStreak;
      } else {
        tempStreak = 0;
      }

      if (weekArr.length === 7) {
        wks.push(weekArr);
        weekArr = [];
      }

      cursor.setDate(cursor.getDate() + 1);
      if (cursor > today && weekArr.length > 0) {
        wks.push(weekArr);
        break;
      }
    }

    // Calculate current streak from today backwards
    let cs = 0;
    const checkDate = new Date(today);
    while (true) {
      const ds = formatDate(checkDate);
      if (doneSet.has(ds)) {
        cs++;
        checkDate.setDate(checkDate.getDate() - 1);
      } else {
        break;
      }
    }

    return { weeks: wks, totalDone: total, longestStreak: longest, currentStreak: cs };
  }, [doneSet]);

  const dayLabels = ['一', '二', '三', '四', '五', '六', '日'];

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm" onClick={onClose}>
      <div
        className="w-full max-w-lg mx-4 bg-[var(--surface-0)] backdrop-blur-xl rounded-2xl border border-[var(--border-default)] p-6 shadow-xl"
        onClick={e => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between mb-4">
          <h3 className="text-lg font-semibold text-[var(--text-primary)]">打卡历史</h3>
          <button
            onClick={onClose}
            className="p-1.5 rounded-lg hover:bg-[var(--surface-2)] text-[var(--text-secondary)] transition-colors"
          >
            <X size={18} />
          </button>
        </div>

        {/* Habit Filter */}
        <div className="mb-4">
          <SelectField
            value={selectedHabitId}
            onValueChange={setSelectedHabitId}
            options={[
              { value: '', label: '全部' },
              ...activeHabits.map(habit => ({ value: habit.id, label: `${habit.emoji} ${habit.title}` })),
            ]}
            className="w-full text-sm"
            ariaLabel="筛选习惯"
          />
        </div>

        {/* Heatmap Grid */}
        <div className="mb-4 overflow-x-auto">
          <div className="flex gap-0.5">
            {/* Day labels column */}
            <div className="flex flex-col gap-0.5 mr-1">
              {dayLabels.map((label, i) => (
                <div key={i} className="w-3 h-3 flex items-center justify-center text-[9px] text-[var(--text-tertiary)]">
                  {i % 2 === 0 ? label : ''}
                </div>
              ))}
            </div>
            {/* Weeks */}
            {weeks.map((week, wi) => (
              <div key={wi} className="flex flex-col gap-0.5">
                {week.map((day, di) => (
                  <div
                    key={di}
                    className={`w-3 h-3 rounded-sm transition-colors ${
                      day.done
                        ? 'bg-emerald-500'
                        : 'bg-gray-200 dark:bg-gray-700'
                    }`}
                    title={`${day.dateStr}${day.done ? ' ✓' : ''}`}
                  />
                ))}
                {/* Pad incomplete weeks */}
                {week.length < 7 && Array.from({ length: 7 - week.length }).map((_, pi) => (
                  <div key={`pad-${pi}`} className="w-3 h-3 rounded-sm bg-transparent" />
                ))}
              </div>
            ))}
          </div>
        </div>

        {/* Stats */}
        <div className="grid grid-cols-3 gap-3">
          <div className="text-center p-3 rounded-xl bg-[var(--surface-1)]">
            <div className="text-xl font-bold text-[var(--text-primary)]">{totalDone}</div>
            <div className="text-xs text-[var(--text-secondary)]">总打卡次数</div>
          </div>
          <div className="text-center p-3 rounded-xl bg-[var(--surface-1)]">
            <div className="text-xl font-bold text-[var(--text-primary)]">{longestStreak}</div>
            <div className="text-xs text-[var(--text-secondary)]">最长连续</div>
          </div>
          <div className="text-center p-3 rounded-xl bg-[var(--surface-1)]">
            <div className="text-xl font-bold text-[var(--text-primary)]">{currentStreak}</div>
            <div className="text-xs text-[var(--text-secondary)]">当前连续</div>
          </div>
        </div>

        {/* Per-habit streak if filtered */}
        {selectedHabitId && (() => {
          const habit = habits.find(h => h.id === selectedHabitId);
          if (!habit) return null;
          return (
            <div className="mt-3 text-center text-sm text-[var(--text-secondary)]">
              当前连续: {getStreak(selectedHabitId)} {habit.frequency.type === 'weekly' ? '周' : '天'}
            </div>
          );
        })()}
      </div>
    </div>
  );
}
