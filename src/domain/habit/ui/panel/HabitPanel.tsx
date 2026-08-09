import { useState, useMemo } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { Plus, History, Sparkles, X, Check, Trash2, Archive, Pencil, LoaderCircle, Flame, ListChecks } from 'lucide-react';
import type { HabitItem, HabitFrequency } from '../../types';
import { useHabits } from '../../state/store';
import { computeWeeklyProgress, formatFrequency, streakUnit } from '../../logic';
import { formatDate } from '@/core/utils/date';
import { showToast, IconButton, ConfirmDialog } from '@/shell';
import HabitHistory from '../components/HabitHistory';

const DAY_LABELS = ['一', '二', '三', '四', '五', '六', '日'];

type FreqType = 'daily' | 'weekly' | 'weekdays';

interface FormState {
  emoji: string;
  title: string;
  freqType: FreqType;
  timesPerWeek: number;
  selectedDays: number[];
}

const EMPTY_FORM: FormState = {
  emoji: '',
  title: '',
  freqType: 'daily',
  timesPerWeek: 3,
  selectedDays: [],
};

function formToFrequency(form: FormState): HabitFrequency {
  switch (form.freqType) {
    case 'daily':
      return { type: 'daily' };
    case 'weekly':
      return { type: 'weekly', timesPerWeek: form.timesPerWeek };
    case 'weekdays':
      return { type: 'weekdays', days: form.selectedDays };
  }
}

function habitToForm(habit: HabitItem): FormState {
  const base: FormState = { ...EMPTY_FORM, emoji: habit.emoji, title: habit.title };
  switch (habit.frequency.type) {
    case 'daily':
      return { ...base, freqType: 'daily' };
    case 'weekly':
      return { ...base, freqType: 'weekly', timesPerWeek: habit.frequency.timesPerWeek };
    case 'weekdays':
      return { ...base, freqType: 'weekdays', selectedDays: habit.frequency.days };
  }
}

/** Stable per-habit gradient (from the design token chart gradients) for the emoji tile. */
function habitGradient(id: string): string {
  let hash = 0;
  for (let i = 0; i < id.length; i++) {
    hash = (hash * 31 + id.charCodeAt(i)) >>> 0;
  }
  return `var(--chart-gradient-${hash % 8})`;
}

interface RenderedHabit {
  habit: HabitItem;
  due: boolean;
  done: boolean;
  streak: number;
  weekStatus: boolean[];
  weekProgress: number | null;
}

function HabitCard({
  item,
  celebrating,
  onToggle,
  onCelebrated,
  onEdit,
  onArchive,
  onDelete,
}: {
  item: RenderedHabit;
  celebrating: boolean;
  onToggle: () => void;
  onCelebrated: () => void;
  onEdit: () => void;
  onArchive: () => void;
  onDelete: () => void;
}) {
  const { habit, due, done, streak, weekStatus, weekProgress } = item;
  const unit = streakUnit(habit.frequency);
  const weeklyTarget = habit.frequency.type === 'weekly' ? habit.frequency.timesPerWeek : null;

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: due ? 1 : 0.45, y: 0 }}
      exit={{ opacity: 0, scale: 0.98, transition: { duration: 0.15, ease: 'easeOut' } }}
      className={`group relative flex items-center gap-3 rounded-[14px] border px-3 py-2.5 transition-colors ${
        done
          ? 'border-emerald-200/70 bg-emerald-50/50 dark:border-emerald-500/15 dark:bg-emerald-500/5'
          : 'border-[var(--border-subtle)] bg-[var(--surface-3)] hover:border-[var(--border-hover)] dark:bg-[var(--surface-1)]'
      }`}
    >
      {/* Emoji tile */}
      <div
        className="w-9 h-9 rounded-[10px] flex items-center justify-center text-[17px] flex-shrink-0 shadow-sm"
        style={{ background: habitGradient(habit.id) }}
      >
        {habit.emoji || '🎯'}
      </div>

      {/* Title + meta */}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="text-[13px] font-medium text-[var(--text-primary)] truncate">{habit.title}</span>
          {streak >= 2 && (
            <span
              title={`连续 ${streak} ${unit === 'week' ? '周' : '天'}`}
              className="flex-shrink-0 inline-flex items-center gap-0.5 rounded-full bg-amber-50 dark:bg-amber-500/10 px-1.5 py-px text-[10px] font-semibold text-amber-600 dark:text-amber-400"
            >
              <Flame size={10} fill="currentColor" />
              {streak}{unit === 'week' ? '周' : '天'}
            </span>
          )}
        </div>
        <div className="mt-0.5 flex items-center gap-2">
          <span className="text-[11px] text-[var(--text-muted)]">{formatFrequency(habit.frequency)}</span>
          {weeklyTarget !== null && weekProgress !== null && (
            <span className="flex items-center gap-1.5">
              <span className="w-14 h-1 rounded-full bg-gray-100 dark:bg-white/10 overflow-hidden">
                <motion.span
                  className="block h-full w-full origin-left rounded-full bg-gradient-to-r from-emerald-400 to-emerald-500"
                  animate={{ scaleX: Math.min(1, weekProgress / weeklyTarget) }}
                  transition={{ type: 'spring', stiffness: 160, damping: 22 }}
                />
              </span>
              <span className="text-[10px] tabular-nums text-[var(--text-muted)]">{weekProgress}/{weeklyTarget}</span>
            </span>
          )}
        </div>
      </div>

      {/* Week squares */}
      <div className="flex items-center gap-[3px] flex-shrink-0" title="最近 7 天">
        {weekStatus.map((dayDone, i) => (
          <div
            key={i}
            className={`w-2 h-2 rounded-[3px] transition-colors ${
              dayDone
                ? 'bg-gradient-to-br from-emerald-400 to-emerald-500'
                : 'bg-gray-200/80 dark:bg-white/10'
            } ${i === weekStatus.length - 1 ? 'ring-1 ring-emerald-400/50 ring-offset-1 dark:ring-offset-transparent' : ''}`}
          />
        ))}
      </div>

      {/* Check button + celebration */}
      {due && (
        <span className="relative flex-shrink-0">
          <motion.button
            whileTap={{ scale: 0.8 }}
            onClick={onToggle}
            title={done ? '取消今日打卡' : '今日打卡'}
            className={`w-6 h-6 rounded-full border-[1.5px] flex items-center justify-center transition-all duration-200 ${
              done
                ? 'border-transparent bg-gradient-to-br from-emerald-400 to-emerald-600 shadow-[0_2px_10px_rgba(16,185,129,0.45)]'
                : 'border-gray-300 dark:border-[#3B4049] hover:border-emerald-400 hover:shadow-[0_0_0_3px_rgba(16,185,129,0.12)]'
            }`}
          >
            <AnimatePresence mode="wait">
              {done && (
                <motion.span
                  key="check"
                  initial={{ scale: 0.9, opacity: 0 }}
                  animate={{ scale: 1, opacity: 1 }}
                  exit={{ scale: 0.9, opacity: 0 }}
                  transition={{ type: 'spring', stiffness: 520, damping: 22 }}
                  className="flex"
                >
                  <Check size={13} strokeWidth={3.5} className="text-white" />
                </motion.span>
              )}
            </AnimatePresence>
          </motion.button>
          <AnimatePresence>
            {celebrating && (
              <motion.span
                key="plus-one"
                initial={{ opacity: 0, y: 2 }}
                animate={{ opacity: 1, y: -16 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.65, ease: 'easeOut' }}
                onAnimationComplete={onCelebrated}
                className="pointer-events-none absolute -top-1 right-0 text-[11px] font-bold text-emerald-500"
              >
                +1
              </motion.span>
            )}
          </AnimatePresence>
        </span>
      )}

      <div className="ml-1 flex shrink-0 items-center gap-0.5 border-l border-[var(--border-subtle)] pl-2 opacity-0 transition-opacity duration-150 group-hover:opacity-100 focus-within:opacity-100">
        <button
          type="button"
          onClick={onEdit}
          title="编辑"
          aria-label={`编辑${habit.title}`}
          className="flex h-7 w-7 cursor-pointer items-center justify-center rounded-[7px] text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/25 dark:hover:bg-white/5 dark:hover:text-gray-200"
        >
          <Pencil size={12} aria-hidden="true" />
        </button>
        <button
          type="button"
          onClick={onArchive}
          title="归档"
          aria-label={`归档${habit.title}`}
          className="flex h-7 w-7 cursor-pointer items-center justify-center rounded-[7px] text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/25 dark:hover:bg-white/5 dark:hover:text-gray-200"
        >
          <Archive size={12} aria-hidden="true" />
        </button>
        <button
          type="button"
          onClick={onDelete}
          aria-label={`删除${habit.title}`}
          className="flex h-7 cursor-pointer items-center gap-1 rounded-[7px] px-2 text-[11px] font-medium text-red-500 transition-colors hover:bg-red-50 hover:text-red-600 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-500/25 dark:text-red-400 dark:hover:bg-red-500/10 dark:hover:text-red-300"
        >
          <Trash2 size={12} aria-hidden="true" />删除
        </button>
      </div>
    </motion.div>
  );
}

export default function HabitPanel() {
  const {
    habits,
    toggleToday,
    addHabit,
    removeHabit,
    updateHabit,
    archiveHabit,
    getStreak,
    getWeekStatus,
    isTodayDue,
  } = useHabits();

  const [showAddForm, setShowAddForm] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [showHistory, setShowHistory] = useState(false);
  const [showAiInput, setShowAiInput] = useState(false);
  const [aiInput, setAiInput] = useState('');
  const [aiLoading, setAiLoading] = useState(false);
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [celebratingId, setCelebratingId] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<HabitItem | null>(null);

  const todayStr = formatDate(new Date());

  const patchForm = (patch: Partial<FormState>) => {
    setForm(prev => ({ ...prev, ...patch }));
  };

  const resetForm = () => {
    setForm(EMPTY_FORM);
    setEditingId(null);
    setShowAddForm(false);
  };

  const openEditForm = (habit: HabitItem) => {
    setForm(habitToForm(habit));
    setEditingId(habit.id);
    setShowAddForm(true);
  };

  const formValid =
    form.title.trim().length > 0 &&
    (form.freqType !== 'weekdays' || form.selectedDays.length > 0);

  const handleSave = () => {
    if (!formValid) return;
    const frequency = formToFrequency(form);
    if (editingId) {
      updateHabit(editingId, {
        title: form.title.trim(),
        emoji: form.emoji,
        frequency,
      });
    } else {
      addHabit(form.title.trim(), form.emoji, frequency);
    }
    resetForm();
  };

  const handleAiSubmit = async () => {
    const goal = aiInput.trim();
    if (!goal || aiLoading) return;
    setAiLoading(true);
    try {
      const { parseHabitWithAI } = await import('@/domain/habit/ai/parseHabit');
      const parsed = await parseHabitWithAI(goal);
      if (parsed.length === 0) {
        showToast({ message: 'AI 未能拆解出习惯，请尝试更具体的描述', type: 'error' });
        return;
      }
      parsed.forEach(h => addHabit(h.title, h.emoji, h.frequency));
      showToast({ message: `已添加 ${parsed.length} 个习惯`, type: 'success' });
      setAiInput('');
      setShowAiInput(false);
    } catch (reason) {
      showToast({
        message: reason instanceof Error ? reason.message : 'AI 分解失败',
        type: 'error',
      });
    } finally {
      setAiLoading(false);
    }
  };

  const handleDelete = (habit: HabitItem) => {
    setDeleteTarget(habit);
  };

  const handleArchive = (habit: HabitItem) => {
    archiveHabit(habit.id);
    showToast({ message: `已归档「${habit.title}」`, type: 'success' });
  };

  const handleToggle = (habit: HabitItem, done: boolean) => {
    toggleToday(habit.id);
    if (!done) setCelebratingId(habit.id);
  };

  const toggleDay = (day: number) => {
    patchForm({
      selectedDays: form.selectedDays.includes(day)
        ? form.selectedDays.filter(d => d !== day)
        : [...form.selectedDays, day],
    });
  };

  const renderedHabits = useMemo<RenderedHabit[]>(() =>
    habits.map(habit => ({
      habit,
      due: isTodayDue(habit),
      done: habit.records.some(r => r.date === todayStr && r.done),
      streak: getStreak(habit.id),
      weekStatus: getWeekStatus(habit.id),
      weekProgress: habit.frequency.type === 'weekly'
        ? computeWeeklyProgress(habit.records, new Date())
        : null,
    })),
    [habits, isTodayDue, getStreak, getWeekStatus, todayStr]
  );

  const dueToday = renderedHabits.filter(r => r.due);
  const doneToday = dueToday.filter(r => r.done).length;
  const todayPct = dueToday.length > 0 ? (doneToday / dueToday.length) * 100 : 0;

  return (
    <>
      <section className="flex h-full min-h-0 flex-col px-4 py-4">
        <div className="mx-auto flex w-full max-w-[960px] min-h-0 flex-1 flex-col">
        {habits.length > 0 && (
          <div className="mb-3 flex min-h-8 flex-wrap items-center justify-end gap-1.5">
            <button
              type="button"
              onClick={() => setShowHistory(true)}
              className="flex h-8 cursor-pointer items-center gap-1.5 rounded-[9px] px-2.5 text-[12px] font-medium text-slate-600 transition-colors hover:bg-slate-100 hover:text-slate-800 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/25 dark:text-[#9CA3AF] dark:hover:bg-[#1A1D24] dark:hover:text-[#D1D5DB]"
            >
              <History size={14} aria-hidden="true" />历史记录
            </button>
            <button
              type="button"
              onClick={() => setShowAiInput(!showAiInput)}
              aria-expanded={showAiInput}
              className="flex h-8 cursor-pointer items-center gap-1.5 rounded-[9px] px-2.5 text-[12px] font-medium text-slate-600 transition-colors hover:bg-slate-100 hover:text-slate-800 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/25 dark:text-[#9CA3AF] dark:hover:bg-[#1A1D24] dark:hover:text-[#D1D5DB]"
            >
              <Sparkles size={14} aria-hidden="true" />从目标生成
            </button>
            <button
              type="button"
              onClick={() => (showAddForm ? resetForm() : setShowAddForm(true))}
              className="flex h-8 cursor-pointer items-center gap-1.5 rounded-[9px] bg-brand px-3 text-[12px] font-medium text-white transition-colors hover:bg-brand-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/30 focus-visible:ring-offset-2 dark:focus-visible:ring-offset-[#0F1117]"
            >
              <Plus size={14} aria-hidden="true" />新建习惯
            </button>
          </div>
        )}

        {/* Today progress */}
        {dueToday.length > 0 && !showAddForm && (
          <div className="mb-3">
            <div className="mb-1 flex items-baseline justify-between">
              <span className="text-[11px] text-[var(--text-muted)]">今日打卡</span>
              <span className={`text-[11px] tabular-nums font-medium ${
                doneToday === dueToday.length ? 'text-emerald-500' : 'text-[var(--text-secondary)]'
              }`}>
                {doneToday === dueToday.length ? '🎉 ' : ''}{doneToday}/{dueToday.length}
              </span>
            </div>
            <div className="h-1.5 rounded-full bg-gray-100 dark:bg-white/5 overflow-hidden">
              <motion.div
                className="h-full w-full origin-left rounded-full bg-gradient-to-r from-emerald-400 to-emerald-500"
                animate={{ scaleX: todayPct / 100 }}
                transition={{ type: 'spring', stiffness: 120, damping: 20 }}
              />
            </div>
          </div>
        )}

        {/* AI Input */}
        {showAiInput && (
          <div className="mb-4 flex gap-2">
            <input
              type="text"
              value={aiInput}
              onChange={e => setAiInput(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && void handleAiSubmit()}
              placeholder="描述你的习惯目标..."
              disabled={aiLoading}
              className="flex-1 px-3 py-1.5 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-[var(--text-primary)] placeholder:text-gray-400 dark:placeholder:text-[#4B5563] outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors disabled:opacity-50"
              autoFocus
            />
            <button
              onClick={() => void handleAiSubmit()}
              disabled={!aiInput.trim() || aiLoading}
              className="px-3 py-1.5 rounded-[8px] bg-brand text-white text-[13px] font-medium disabled:opacity-40 transition-opacity flex items-center gap-1.5"
            >
              {aiLoading && <LoaderCircle size={12} className="animate-spin" />}
              分解
            </button>
            <IconButton title="关闭" onClick={() => { setShowAiInput(false); setAiInput(''); }} disabled={aiLoading}>
              <X size={14} />
            </IconButton>
          </div>
        )}

        {/* Add / Edit Form */}
        {showAddForm && (
          <div className="mb-4 space-y-2">
            <div className="flex gap-2">
              <input
                type="text"
                value={form.emoji}
                onChange={e => patchForm({ emoji: e.target.value })}
                placeholder="🎯"
                className="w-12 px-2 py-1.5 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-[var(--text-primary)] text-center outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors"
                maxLength={4}
              />
              <input
                type="text"
                value={form.title}
                onChange={e => patchForm({ title: e.target.value })}
                onKeyDown={e => e.key === 'Enter' && handleSave()}
                placeholder="习惯名称"
                className="flex-1 px-3 py-1.5 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-[var(--text-primary)] placeholder:text-gray-400 dark:placeholder:text-[#4B5563] outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors"
                autoFocus
              />
            </div>

            {/* Frequency selector */}
            <div className="flex gap-1">
              {(['daily', 'weekly', 'weekdays'] as const).map(ft => (
                <button
                  key={ft}
                  onClick={() => patchForm({ freqType: ft })}
                  className={`h-7 px-2.5 rounded-[8px] text-[11px] font-medium transition-colors ${
                    form.freqType === ft
                      ? 'bg-brand text-white'
                      : 'bg-gray-50 dark:bg-[#1A1D24] text-[var(--text-secondary)] border border-gray-200 dark:border-[#2A2D35] hover:text-gray-600 dark:hover:text-[#9CA3AF]'
                  }`}
                >
                  {ft === 'daily' ? '每天' : ft === 'weekly' ? '每周X次' : '指定星期'}
                </button>
              ))}
            </div>

            {/* Weekly: times per week */}
            {form.freqType === 'weekly' && (
              <div className="flex items-center gap-2">
                <span className="text-[11px] text-[var(--text-secondary)]">每周</span>
                <input
                  type="number"
                  min={1}
                  max={7}
                  value={form.timesPerWeek}
                  onChange={e => patchForm({ timesPerWeek: Math.min(7, Math.max(1, Number(e.target.value))) })}
                  className="w-12 px-2 py-1 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-[var(--text-primary)] text-center outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors"
                />
                <span className="text-[11px] text-[var(--text-secondary)]">次</span>
              </div>
            )}

            {/* Weekdays: day picker */}
            {form.freqType === 'weekdays' && (
              <div className="flex gap-1">
                {DAY_LABELS.map((label, i) => {
                  // Map display index to JS day: 一=1,二=2,...,日=0
                  const jsDay = i === 6 ? 0 : i + 1;
                  const isSelected = form.selectedDays.includes(jsDay);
                  return (
                    <button
                      key={i}
                      onClick={() => toggleDay(jsDay)}
                      className={`w-6 h-6 rounded-[6px] text-[11px] font-medium transition-colors ${
                        isSelected
                          ? 'bg-brand text-white'
                          : 'bg-gray-50 dark:bg-[#1A1D24] text-[var(--text-secondary)] border border-gray-200 dark:border-[#2A2D35] hover:text-gray-600 dark:hover:text-[#9CA3AF]'
                      }`}
                    >
                      {label}
                    </button>
                  );
                })}
              </div>
            )}
            {form.freqType === 'weekdays' && form.selectedDays.length === 0 && (
              <p className="text-[11px] text-[#F59E0B]">请至少选择一天</p>
            )}

            {/* Actions */}
            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={resetForm}
                className="px-3 py-1.5 rounded-[8px] text-[11px] text-[var(--text-secondary)] hover:text-gray-600 dark:hover:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors"
              >
                取消
              </button>
              <button
                onClick={handleSave}
                disabled={!formValid}
                className="px-3 py-1.5 rounded-[8px] bg-brand text-white text-[11px] font-medium disabled:opacity-40 transition-opacity"
              >
                {editingId ? '保存' : '确认'}
              </button>
            </div>
          </div>
        )}

        {/* Habit List */}
        <div className="min-h-0 flex-1">
          {habits.length === 0 && !showAddForm && !showAiInput && (
            <div className="flex h-full min-h-[360px] items-center justify-center pb-[6vh]">
              <div className="grid w-full max-w-[720px] grid-cols-[minmax(0,1fr)_300px] gap-8 rounded-[16px] border border-slate-200/80 bg-slate-50/80 p-8 dark:border-[#2A2D35] dark:bg-[#151820]">
                <div className="min-w-0">
                  <div className="flex items-start gap-4">
                  <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-[12px] bg-emerald-100 text-emerald-700 dark:bg-emerald-500/10 dark:text-emerald-400">
                    <ListChecks size={21} aria-hidden="true" />
                  </div>
                  <div className="min-w-0 pt-0.5">
                    <h3 className="text-[15px] font-semibold tracking-[-0.01em] text-slate-900 dark:text-[#E5E7EB]">
                      从一个小行动开始
                    </h3>
                    <p className="mt-1.5 max-w-[390px] text-[12px] leading-5 text-slate-600 dark:text-[#9CA3AF]">
                      创建后即可按计划打卡，并查看连续完成情况。
                    </p>
                  </div>
                </div>
                  <div className="mt-6 flex items-center gap-2">
                    <button
                      type="button"
                      onClick={() => setShowAddForm(true)}
                      className="flex h-8 shrink-0 cursor-pointer items-center justify-center gap-1.5 rounded-[9px] bg-brand px-3 text-[12px] font-medium text-white transition-colors hover:bg-brand-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/30 focus-visible:ring-offset-2 dark:focus-visible:ring-offset-[#151820]"
                    >
                      <Plus size={14} aria-hidden="true" />新建习惯
                    </button>
                    <button
                      type="button"
                      onClick={() => setShowAiInput(true)}
                      className="flex h-8 cursor-pointer items-center gap-1.5 rounded-[9px] px-2.5 text-[12px] font-medium text-slate-600 transition-colors hover:bg-white hover:text-slate-900 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/25 dark:text-[#9CA3AF] dark:hover:bg-white/5 dark:hover:text-[#D1D5DB]"
                    >
                      <Sparkles size={14} aria-hidden="true" />从目标生成
                    </button>
                  </div>
                </div>

                <div className="overflow-hidden self-center rounded-[12px] border border-slate-200/90 bg-white dark:border-[#2A2D35] dark:bg-[#101319]" aria-label="习惯创建后的示例">
                <div className="flex items-center gap-3 px-3.5 py-3">
                  <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-slate-300 dark:border-[#3B4049]" aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-slate-700 dark:text-[#D1D5DB]">阅读 20 分钟</span>
                  <span className="shrink-0 text-[11px] text-slate-500 dark:text-[#7D8590]">每天</span>
                </div>
                <div className="border-t border-slate-100 dark:border-[#20242C]" />
                <div className="flex items-center gap-3 px-3.5 py-3">
                  <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-slate-300 dark:border-[#3B4049]" aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-slate-700 dark:text-[#D1D5DB]">运动 30 分钟</span>
                  <span className="shrink-0 text-[11px] text-slate-500 dark:text-[#7D8590]">周一至周五</span>
                </div>
              </div>
              </div>
            </div>
          )}
          <div className="grid grid-cols-1 items-start gap-1.5 lg:grid-cols-2">
            <AnimatePresence initial={false}>
              {renderedHabits.map(item => (
                <HabitCard
                  key={item.habit.id}
                  item={item}
                  celebrating={celebratingId === item.habit.id}
                  onToggle={() => handleToggle(item.habit, item.done)}
                  onCelebrated={() => setCelebratingId(null)}
                  onEdit={() => openEditForm(item.habit)}
                  onArchive={() => handleArchive(item.habit)}
                  onDelete={() => handleDelete(item.habit)}
                />
              ))}
            </AnimatePresence>
          </div>
        </div>
        </div>
      </section>

      {/* History Modal */}
      <AnimatePresence>
        {showHistory && (
          <HabitHistory
            habits={habits}
            getStreak={getStreak}
            onClose={() => setShowHistory(false)}
          />
        )}
      </AnimatePresence>

      <ConfirmDialog
        open={deleteTarget !== null}
        title="删除习惯"
        description={deleteTarget ? `确定删除习惯「${deleteTarget.title}」？此操作不可恢复。` : ''}
        onConfirm={() => {
          if (deleteTarget) removeHabit(deleteTarget.id);
          setDeleteTarget(null);
        }}
        onClose={() => setDeleteTarget(null)}
      />
    </>
  );
}
