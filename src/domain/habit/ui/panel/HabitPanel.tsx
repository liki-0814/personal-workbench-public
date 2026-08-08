import { useState, useMemo } from 'react';
import { Plus, History, Sparkles, X, Check, Trash2, Archive, Pencil, LoaderCircle } from 'lucide-react';
import type { HabitItem, HabitFrequency } from '../../types';
import { useHabits } from '../../state/store';
import { streakUnit } from '../../logic';
import { formatDate } from '@/core/utils/date';
import { showToast, IconButton, PanelHeader } from '@/shell';
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
    if (!window.confirm(`确定删除习惯「${habit.title}」？此操作不可恢复。`)) return;
    removeHabit(habit.id);
  };

  const handleArchive = (habit: HabitItem) => {
    archiveHabit(habit.id);
    showToast({ message: `已归档「${habit.title}」`, type: 'success' });
  };

  const toggleDay = (day: number) => {
    patchForm({
      selectedDays: form.selectedDays.includes(day)
        ? form.selectedDays.filter(d => d !== day)
        : [...form.selectedDays, day],
    });
  };

  const renderedHabits = useMemo(() =>
    habits.map(habit => ({
      habit,
      due: isTodayDue(habit),
      done: habit.records.some(r => r.date === todayStr && r.done),
      streak: getStreak(habit.id),
      weekStatus: getWeekStatus(habit.id),
    })),
    [habits, isTodayDue, getStreak, getWeekStatus, todayStr]
  );

  return (
    <>
      <section className="px-4 py-4">
        <PanelHeader
          title="习惯"
          actions={
            <>
              <IconButton title="AI 分解习惯" onClick={() => setShowAiInput(!showAiInput)}>
                <Sparkles size={14} />
              </IconButton>
              <IconButton title="历史" onClick={() => setShowHistory(true)}>
                <History size={14} />
              </IconButton>
              <IconButton title="添加习惯" onClick={() => (showAddForm ? resetForm() : setShowAddForm(true))}>
                <Plus size={14} />
              </IconButton>
            </>
          }
        />

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
              className="flex-1 px-3 py-1.5 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-gray-900 dark:text-[#E5E7EB] placeholder:text-gray-400 dark:placeholder:text-[#4B5563] outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors disabled:opacity-50"
              autoFocus
            />
            <button
              onClick={() => void handleAiSubmit()}
              disabled={!aiInput.trim() || aiLoading}
              className="px-3 py-1.5 rounded-[8px] bg-[#3B82F6] text-white text-[13px] font-medium disabled:opacity-40 transition-opacity flex items-center gap-1.5"
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
                className="w-12 px-2 py-1.5 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-gray-900 dark:text-[#E5E7EB] text-center outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors"
                maxLength={4}
              />
              <input
                type="text"
                value={form.title}
                onChange={e => patchForm({ title: e.target.value })}
                onKeyDown={e => e.key === 'Enter' && handleSave()}
                placeholder="习惯名称"
                className="flex-1 px-3 py-1.5 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-gray-900 dark:text-[#E5E7EB] placeholder:text-gray-400 dark:placeholder:text-[#4B5563] outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors"
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
                      ? 'bg-[#3B82F6] text-white'
                      : 'bg-gray-50 dark:bg-[#1A1D24] text-gray-500 dark:text-[#6B7280] border border-gray-200 dark:border-[#2A2D35] hover:text-gray-600 dark:hover:text-[#9CA3AF]'
                  }`}
                >
                  {ft === 'daily' ? '每天' : ft === 'weekly' ? '每周X次' : '指定星期'}
                </button>
              ))}
            </div>

            {/* Weekly: times per week */}
            {form.freqType === 'weekly' && (
              <div className="flex items-center gap-2">
                <span className="text-[11px] text-gray-500 dark:text-[#6B7280]">每周</span>
                <input
                  type="number"
                  min={1}
                  max={7}
                  value={form.timesPerWeek}
                  onChange={e => patchForm({ timesPerWeek: Math.min(7, Math.max(1, Number(e.target.value))) })}
                  className="w-12 px-2 py-1 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-gray-900 dark:text-[#E5E7EB] text-center outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors"
                />
                <span className="text-[11px] text-gray-500 dark:text-[#6B7280]">次</span>
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
                          ? 'bg-[#3B82F6] text-white'
                          : 'bg-gray-50 dark:bg-[#1A1D24] text-gray-500 dark:text-[#6B7280] border border-gray-200 dark:border-[#2A2D35] hover:text-gray-600 dark:hover:text-[#9CA3AF]'
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
                className="px-3 py-1.5 rounded-[8px] text-[11px] text-gray-500 dark:text-[#6B7280] hover:text-gray-600 dark:hover:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors"
              >
                取消
              </button>
              <button
                onClick={handleSave}
                disabled={!formValid}
                className="px-3 py-1.5 rounded-[8px] bg-[#3B82F6] text-white text-[11px] font-medium disabled:opacity-40 transition-opacity"
              >
                {editingId ? '保存' : '确认'}
              </button>
            </div>
          </div>
        )}

        {/* Habit List */}
        <div>
          {habits.length === 0 && !showAddForm && (
            <div className="py-4 text-[13px] text-gray-400 dark:text-[#4B5563]">
              暂无习惯
            </div>
          )}
          {renderedHabits.map(({ habit, due, done, streak, weekStatus }) => {
            const unit = streakUnit(habit.frequency);
            const showStreak = unit === 'week' ? streak >= 2 : streak >= 7;

            return (
              <div
                key={habit.id}
                className={`group flex items-center gap-3 py-2 px-2 rounded-[8px] hover:bg-gray-100 dark:hover:bg-[#1A1D24]/50 transition-all ${
                  !due ? 'opacity-40' : ''
                }`}
              >
                {/* Emoji + Title */}
                <div className="flex items-center gap-2 min-w-0 flex-1">
                  <span className="text-base flex-shrink-0">{habit.emoji}</span>
                  <span className="text-[13px] text-gray-900 dark:text-[#E5E7EB] truncate">{habit.title}</span>
                  {showStreak && (
                    <span className="flex-shrink-0 text-[10px] font-medium text-[#F59E0B]">
                      {streak}{unit === 'week' ? 'w' : 'd'}
                    </span>
                  )}
                </div>

                {/* Week dots */}
                <div className="flex items-center gap-1 flex-shrink-0">
                  {weekStatus.map((dayDone, i) => (
                    <div
                      key={i}
                      className={`w-1.5 h-1.5 rounded-full transition-colors ${
                        dayDone ? 'bg-[#10B981]' : 'bg-gray-200 dark:bg-[#2A2D35]'
                      }`}
                    />
                  ))}
                </div>

                {/* Today check button */}
                {due && (
                  <button
                    onClick={() => toggleToday(habit.id)}
                    className={`w-5 h-5 rounded-[6px] border flex items-center justify-center flex-shrink-0 transition-all duration-200 ${
                      done
                        ? 'bg-[#10B981] border-[#10B981]'
                        : 'border-gray-200 dark:border-[#2A2D35] hover:border-[#10B981]/50'
                    }`}
                  >
                    {done && <Check size={12} className="text-white" />}
                  </button>
                )}

                {/* Row actions (hover only) */}
                <button
                  onClick={() => openEditForm(habit)}
                  className="w-5 h-5 flex items-center justify-center flex-shrink-0 opacity-0 group-hover:opacity-100 text-gray-400 dark:text-[#4B5563] hover:text-gray-600 dark:hover:text-[#9CA3AF] transition-all"
                  title="编辑"
                >
                  <Pencil size={12} />
                </button>
                <button
                  onClick={() => handleArchive(habit)}
                  className="w-5 h-5 flex items-center justify-center flex-shrink-0 opacity-0 group-hover:opacity-100 text-gray-400 dark:text-[#4B5563] hover:text-gray-600 dark:hover:text-[#9CA3AF] transition-all"
                  title="归档"
                >
                  <Archive size={12} />
                </button>
                <button
                  onClick={() => handleDelete(habit)}
                  className="w-5 h-5 flex items-center justify-center flex-shrink-0 opacity-0 group-hover:opacity-100 text-gray-400 dark:text-[#4B5563] hover:text-[#EF4444] transition-all"
                  title="删除"
                >
                  <Trash2 size={12} />
                </button>
              </div>
            );
          })}
        </div>
      </section>

      {/* History Modal */}
      {showHistory && (
        <HabitHistory
          habits={habits}
          getStreak={getStreak}
          onClose={() => setShowHistory(false)}
        />
      )}
    </>
  );
}
