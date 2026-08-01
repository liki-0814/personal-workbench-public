import { useState, useMemo } from 'react';
import { Plus, History, Sparkles, X, Check, Trash2 } from 'lucide-react';
import type { HabitItem, HabitFrequency } from '../../types';
import { formatDate } from '@/core/utils/date';
import HabitHistory from '../components/HabitHistory';

interface Props {
  habits: HabitItem[];
  onToggleToday: (id: string) => void;
  onAdd: (title: string, emoji: string, frequency: HabitFrequency) => void;
  onRemove: (id: string) => void;
  onUpdate: (id: string, updates: Partial<HabitItem>) => void;
  onArchive: (id: string) => void;
  getStreak: (id: string) => number;
  getWeekStatus: (id: string) => boolean[];
  isTodayDue: (habit: HabitItem) => boolean;
  onAiDecompose?: (goal: string) => void;
}

const DAY_LABELS = ['一', '二', '三', '四', '五', '六', '日'];

export default function HabitPanel({
  habits,
  onToggleToday,
  onAdd,
  onRemove,
  onUpdate: _onUpdate,
  onArchive: _onArchive,
  getStreak,
  getWeekStatus,
  isTodayDue,
  onAiDecompose,
}: Props) {
  const [showAddForm, setShowAddForm] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [showAiInput, setShowAiInput] = useState(false);
  const [aiInput, setAiInput] = useState('');

  // Add form state
  const [newEmoji, setNewEmoji] = useState('');
  const [newTitle, setNewTitle] = useState('');
  const [freqType, setFreqType] = useState<'daily' | 'weekly' | 'weekdays'>('daily');
  const [timesPerWeek, setTimesPerWeek] = useState(3);
  const [selectedDays, setSelectedDays] = useState<number[]>([]);

  const todayStr = formatDate(new Date());

  const resetForm = () => {
    setNewEmoji('');
    setNewTitle('');
    setFreqType('daily');
    setTimesPerWeek(3);
    setSelectedDays([]);
    setShowAddForm(false);
  };

  const handleAdd = () => {
    if (!newTitle.trim()) return;
    let frequency: HabitFrequency;
    switch (freqType) {
      case 'daily':
        frequency = { type: 'daily' };
        break;
      case 'weekly':
        frequency = { type: 'weekly', timesPerWeek };
        break;
      case 'weekdays':
        frequency = { type: 'weekdays', days: selectedDays };
        break;
    }
    onAdd(newTitle.trim(), newEmoji || '', frequency);
    resetForm();
  };

  const handleAiSubmit = () => {
    if (!aiInput.trim() || !onAiDecompose) return;
    onAiDecompose(aiInput.trim());
    setAiInput('');
    setShowAiInput(false);
  };

  const toggleDay = (day: number) => {
    setSelectedDays(prev =>
      prev.includes(day) ? prev.filter(d => d !== day) : [...prev, day]
    );
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
        {/* Header */}
        <div className="flex items-center justify-between pb-3 mb-3 border-b border-gray-100 dark:border-[#1E2028]">
          <h3 className="text-[13px] font-semibold text-gray-900 dark:text-[#E5E7EB] tracking-[-0.01em]">习惯</h3>
          <div className="flex items-center gap-1">
            {onAiDecompose && (
              <button
                onClick={() => setShowAiInput(!showAiInput)}
                className="w-7 h-7 flex items-center justify-center rounded-[8px] text-gray-400 dark:text-[#4B5563] hover:text-gray-600 dark:hover:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors"
                title="AI 分解习惯"
              >
                <Sparkles size={14} />
              </button>
            )}
            <button
              onClick={() => setShowHistory(true)}
              className="w-7 h-7 flex items-center justify-center rounded-[8px] text-gray-400 dark:text-[#4B5563] hover:text-gray-600 dark:hover:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors"
              title="历史"
            >
              <History size={14} />
            </button>
            <button
              onClick={() => setShowAddForm(!showAddForm)}
              className="w-7 h-7 flex items-center justify-center rounded-[8px] text-gray-400 dark:text-[#4B5563] hover:text-gray-600 dark:hover:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors"
              title="添加习惯"
            >
              <Plus size={14} />
            </button>
          </div>
        </div>

        {/* AI Input */}
        {showAiInput && onAiDecompose && (
          <div className="mb-4 flex gap-2">
            <input
              type="text"
              value={aiInput}
              onChange={e => setAiInput(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && handleAiSubmit()}
              placeholder="描述你的习惯目标..."
              className="flex-1 px-3 py-1.5 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-gray-900 dark:text-[#E5E7EB] placeholder:text-gray-400 dark:placeholder:text-[#4B5563] outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors"
              autoFocus
            />
            <button
              onClick={handleAiSubmit}
              disabled={!aiInput.trim()}
              className="px-3 py-1.5 rounded-[8px] bg-[#3B82F6] text-white text-[13px] font-medium disabled:opacity-40 transition-opacity"
            >
              分解
            </button>
            <button
              onClick={() => { setShowAiInput(false); setAiInput(''); }}
              className="w-7 h-7 flex items-center justify-center rounded-[8px] text-gray-400 dark:text-[#4B5563] hover:text-gray-600 dark:hover:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors"
            >
              <X size={14} />
            </button>
          </div>
        )}

        {/* Add Form */}
        {showAddForm && (
          <div className="mb-4 space-y-2">
            <div className="flex gap-2">
              <input
                type="text"
                value={newEmoji}
                onChange={e => setNewEmoji(e.target.value)}
                placeholder="🎯"
                className="w-12 px-2 py-1.5 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-gray-900 dark:text-[#E5E7EB] text-center outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors"
                maxLength={4}
              />
              <input
                type="text"
                value={newTitle}
                onChange={e => setNewTitle(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && handleAdd()}
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
                  onClick={() => setFreqType(ft)}
                  className={`h-7 px-2.5 rounded-[8px] text-[11px] font-medium transition-colors ${
                    freqType === ft
                      ? 'bg-[#3B82F6] text-white'
                      : 'bg-gray-50 dark:bg-[#1A1D24] text-gray-500 dark:text-[#6B7280] border border-gray-200 dark:border-[#2A2D35] hover:text-gray-600 dark:hover:text-[#9CA3AF]'
                  }`}
                >
                  {ft === 'daily' ? '每天' : ft === 'weekly' ? '每周X次' : '指定星期'}
                </button>
              ))}
            </div>

            {/* Weekly: times per week */}
            {freqType === 'weekly' && (
              <div className="flex items-center gap-2">
                <span className="text-[11px] text-gray-500 dark:text-[#6B7280]">每周</span>
                <input
                  type="number"
                  min={1}
                  max={7}
                  value={timesPerWeek}
                  onChange={e => setTimesPerWeek(Math.min(7, Math.max(1, Number(e.target.value))))}
                  className="w-12 px-2 py-1 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-gray-900 dark:text-[#E5E7EB] text-center outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors"
                />
                <span className="text-[11px] text-gray-500 dark:text-[#6B7280]">次</span>
              </div>
            )}

            {/* Weekdays: day picker */}
            {freqType === 'weekdays' && (
              <div className="flex gap-1">
                {DAY_LABELS.map((label, i) => {
                  // Map display index to JS day: 一=1,二=2,...,日=0
                  const jsDay = i === 6 ? 0 : i + 1;
                  const isSelected = selectedDays.includes(jsDay);
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

            {/* Actions */}
            <div className="flex justify-end gap-2 pt-2">
              <button
                onClick={resetForm}
                className="px-3 py-1.5 rounded-[8px] text-[11px] text-gray-500 dark:text-[#6B7280] hover:text-gray-600 dark:hover:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors"
              >
                取消
              </button>
              <button
                onClick={handleAdd}
                disabled={!newTitle.trim()}
                className="px-3 py-1.5 rounded-[8px] bg-[#3B82F6] text-white text-[11px] font-medium disabled:opacity-40 transition-opacity"
              >
                确认
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
                  {streak >= 7 && (
                    <span className="flex-shrink-0 text-[10px] font-medium text-[#F59E0B]">
                      {streak}d
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
                    onClick={() => onToggleToday(habit.id)}
                    className={`w-5 h-5 rounded-[6px] border flex items-center justify-center flex-shrink-0 transition-all duration-200 ${
                      done
                        ? 'bg-[#10B981] border-[#10B981]'
                        : 'border-gray-200 dark:border-[#2A2D35] hover:border-[#10B981]/50'
                    }`}
                  >
                    {done && <Check size={12} className="text-white" />}
                  </button>
                )}

                {/* Delete (hover only) */}
                <button
                  onClick={() => onRemove(habit.id)}
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
