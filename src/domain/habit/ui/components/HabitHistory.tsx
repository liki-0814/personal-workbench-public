import { useState, useMemo } from 'react';
import { motion } from 'framer-motion';
import { X, CheckCircle2, Trophy, Flame, CalendarDays } from 'lucide-react';
import type { HabitItem } from '../../types';
import { formatDate } from '@/core/utils/date';
import { SelectField } from '@/shell';

interface Props {
  habits: HabitItem[];
  getStreak: (id: string) => number;
  onClose: () => void;
}

const WEEKDAY_LABELS = ['一', '二', '三', '四', '五', '六', '日'];
const WEEKS = 14;

/** Emerald intensity level (0-4) for a per-day check-in count. */
function levelClass(count: number): string {
  switch (Math.min(count, 4)) {
    case 0:
      return 'bg-gray-100 dark:bg-white/[0.06]';
    case 1:
      return 'bg-emerald-200 dark:bg-emerald-500/25';
    case 2:
      return 'bg-emerald-300 dark:bg-emerald-500/45';
    case 3:
      return 'bg-emerald-400 dark:bg-emerald-500/65';
    default:
      return 'bg-emerald-500 dark:bg-emerald-400';
  }
}

function StatCard({ icon, value, label, tint }: {
  icon: React.ReactNode;
  value: number;
  label: string;
  tint: string;
}) {
  return (
    <div className={`flex items-center gap-2.5 rounded-[12px] px-3 py-2.5 ${tint}`}>
      <span className="flex-shrink-0">{icon}</span>
      <span className="min-w-0">
        <span className="block text-[17px] font-bold leading-tight tabular-nums text-gray-900 dark:text-[#E5E7EB]">{value}</span>
        <span className="block text-[10px] text-gray-500 dark:text-[#9CA3AF]">{label}</span>
      </span>
    </div>
  );
}

export default function HabitHistory({ habits, getStreak, onClose }: Props) {
  const [selectedHabitId, setSelectedHabitId] = useState<string | ''>('');

  const activeHabits = useMemo(() => habits.filter(h => !h.archived), [habits]);
  const selectedHabit = selectedHabitId ? habits.find(h => h.id === selectedHabitId) : undefined;

  // Per-date check-in counts (sums across habits when unfiltered)
  const countByDate = useMemo(() => {
    const map = new Map<string, number>();
    const source = selectedHabit ? [selectedHabit] : activeHabits;
    for (const habit of source) {
      for (const rec of habit.records) {
        if (rec.done) map.set(rec.date, (map.get(rec.date) ?? 0) + 1);
      }
    }
    return map;
  }, [activeHabits, selectedHabit]);

  // Generate N weeks of dates (columns = weeks, rows = Mon-Sun)
  const { weeks, totalDone, longestStreak, currentStreak } = useMemo(() => {
    const today = new Date();
    const startDate = new Date(today);
    const dayOfWeek = today.getDay(); // 0=Sun
    const mondayOffset = dayOfWeek === 0 ? 6 : dayOfWeek - 1;
    startDate.setDate(today.getDate() - mondayOffset - (WEEKS - 1) * 7);

    const wks: { dateStr: string; count: number; inFuture: boolean }[][] = [];
    let total = 0;
    let longest = 0;
    let tempStreak = 0;

    const cursor = new Date(startDate);
    let weekArr: { dateStr: string; count: number; inFuture: boolean }[] = [];

    while (true) {
      const inFuture = cursor > today;
      const dateStr = formatDate(cursor);
      const count = inFuture ? 0 : (countByDate.get(dateStr) ?? 0);
      weekArr.push({ dateStr, count, inFuture });

      if (count > 0) {
        total += count;
        tempStreak++;
        if (tempStreak > longest) longest = tempStreak;
      } else if (!inFuture) {
        tempStreak = 0;
      }

      if (weekArr.length === 7) {
        wks.push(weekArr);
        weekArr = [];
        if (cursor > today) break;
      }
      cursor.setDate(cursor.getDate() + 1);
    }

    // Current streak: consecutive days with any check-in, today allowed to be pending
    const check = new Date(today);
    if (!(countByDate.get(formatDate(check)) ?? 0)) {
      check.setDate(check.getDate() - 1);
    }
    let cs = 0;
    while ((countByDate.get(formatDate(check)) ?? 0) > 0) {
      cs++;
      check.setDate(check.getDate() - 1);
    }

    return { weeks: wks, totalDone: total, longestStreak: longest, currentStreak: cs };
  }, [countByDate]);

  // Month labels above the columns that start a new month
  const monthLabels = useMemo(() => {
    let prevMonth = -1;
    return weeks.map(week => {
      const first = week[0];
      const month = new Date(first.dateStr + 'T00:00:00').getMonth();
      if (month !== prevMonth) {
        prevMonth = month;
        return `${month + 1}月`;
      }
      return '';
    });
  }, [weeks]);

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.18, ease: 'easeOut' }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 backdrop-blur-sm"
      onClick={onClose}
    >
      <motion.div
        initial={{ opacity: 0, scale: 0.96, y: 10 }}
        animate={{ opacity: 1, scale: 1, y: 0 }}
        exit={{ opacity: 0, scale: 0.97, transition: { duration: 0.15, ease: 'easeOut' } }}
        transition={{ type: 'spring', stiffness: 320, damping: 26 }}
        className="w-full max-w-[420px] mx-4 rounded-2xl border border-[var(--border-default)] bg-[var(--surface-0)] p-5 shadow-2xl"
        onClick={e => e.stopPropagation()}
      >
        {/* Header */}
        <div className="mb-4 flex items-center justify-between">
          <div className="flex items-center gap-2.5">
            <span className="w-8 h-8 rounded-[10px] flex items-center justify-center text-white shadow-sm" style={{ background: 'var(--chart-gradient-1)' }}>
              <CalendarDays size={15} />
            </span>
            <div>
              <h3 className="text-[15px] font-semibold text-[var(--text-primary)]">打卡历史</h3>
              <p className="text-[10px] text-[var(--text-secondary)]">最近 {WEEKS} 周</p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="w-7 h-7 flex items-center justify-center rounded-[8px] text-[var(--text-secondary)] hover:bg-[var(--surface-2)] transition-colors"
            title="关闭"
          >
            <X size={16} />
          </button>
        </div>

        {/* Habit Filter */}
        <div className="mb-4">
          <SelectField
            value={selectedHabitId}
            onValueChange={setSelectedHabitId}
            options={[
              { value: '', label: '全部习惯' },
              ...activeHabits.map(habit => ({ value: habit.id, label: `${habit.emoji} ${habit.title}` })),
            ]}
            className="w-full text-sm"
            ariaLabel="筛选习惯"
          />
        </div>

        {/* Heatmap */}
        <div className="mb-4 flex justify-center">
          <div className="flex gap-[3px]">
            {/* Weekday labels column */}
            <div className="mr-1 flex flex-col gap-[3px] pt-[18px]">
              {WEEKDAY_LABELS.map((label, i) => (
                <div key={i} className="w-4 h-4 flex items-center justify-center text-[9px] text-[var(--text-muted)]">
                  {i % 2 === 0 ? label : ''}
                </div>
              ))}
            </div>
            {weeks.map((week, wi) => (
              <div key={wi} className="flex flex-col gap-[3px]">
                <div className="h-[15px] text-[9px] leading-[15px] text-[var(--text-muted)] whitespace-nowrap">
                  {monthLabels[wi]}
                </div>
                {week.map((day, di) => (
                  <div
                    key={di}
                    className={`w-4 h-4 rounded-[4px] transition-shadow hover:ring-1 hover:ring-emerald-400/60 ${
                      day.inFuture ? 'bg-transparent' : levelClass(day.count)
                    }`}
                    title={day.inFuture ? day.dateStr : `${day.dateStr} · ${day.count} 次打卡`}
                  />
                ))}
              </div>
            ))}
          </div>
        </div>

        {/* Legend */}
        <div className="mb-4 flex items-center justify-end gap-1 text-[9px] text-[var(--text-muted)]">
          少
          {[0, 1, 2, 3, 4].map(level => (
            <span key={level} className={`w-2.5 h-2.5 rounded-[3px] ${levelClass(level)}`} />
          ))}
          多
        </div>

        {/* Stats */}
        <div className="grid grid-cols-3 gap-2">
          <StatCard
            icon={<CheckCircle2 size={17} className="text-emerald-500" />}
            value={totalDone}
            label="总打卡次数"
            tint="bg-emerald-50 dark:bg-emerald-500/10"
          />
          <StatCard
            icon={<Trophy size={17} className="text-violet-500" />}
            value={longestStreak}
            label="最长连续(天)"
            tint="bg-violet-50 dark:bg-violet-500/10"
          />
          <StatCard
            icon={<Flame size={17} className="text-amber-500" />}
            value={currentStreak}
            label="当前连续(天)"
            tint="bg-amber-50 dark:bg-amber-500/10"
          />
        </div>

        {/* Per-habit streak if filtered */}
        {selectedHabit && (
          <div className="mt-3 flex items-center justify-center gap-1.5 rounded-full bg-amber-50 dark:bg-amber-500/10 py-1.5 text-[12px] font-medium text-amber-600 dark:text-amber-400">
            <Flame size={13} fill="currentColor" />
            当前连续 {getStreak(selectedHabit.id)} {selectedHabit.frequency.type === 'weekly' ? '周' : '天'}
          </div>
        )}
      </motion.div>
    </motion.div>
  );
}
