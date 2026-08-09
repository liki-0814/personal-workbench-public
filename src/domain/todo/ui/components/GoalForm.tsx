import { useState } from 'react';
import { Check, ChevronDown, Edit3, Plus, Sparkles, Trash2, X } from 'lucide-react';
import { generateId } from '@/core/utils/id';
import type { Goal, KeyResult } from '../../types';
import DateInput from './DateInput';

interface GoalDraft {
  title: string;
  description?: string;
  deadline?: string;
  periodStart?: string;
  periodEnd?: string;
  keyResults?: Goal['keyResults'];
}

interface Props {
  goal?: Goal;
  goals?: Goal[];
  onSave: (data: GoalDraft) => void;
  onUpdate?: (id: string, data: GoalDraft) => void;
  onDelete?: (id: string) => void;
  onCancel: () => void;
  onAiCreate?: () => void;
}

const INPUT_CLASS = 'w-full rounded-[7px] border border-[#D9DDE4] bg-white px-3 text-[#171A1F] outline-none transition-[border-color,box-shadow,background-color] placeholder:text-[#9AA1AC] focus:border-[#B8BEC8] focus:shadow-[0_0_0_3px_rgba(32,36,43,0.05)] dark:border-[#363D47] dark:bg-[#181B21] dark:text-[#E9ECF1] dark:focus:border-[#59616D] dark:focus:shadow-[0_0_0_3px_rgba(233,236,241,0.05)]';
const KEYBOARD_FOCUS = 'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#9AA1AC]/45 focus-visible:ring-offset-2 dark:focus-visible:ring-offset-[#14171C]';

function createKeyResult(title = '', index = 0): KeyResult {
  return {
    id: `kr-${generateId()}`,
    code: `KR${index + 1}`,
    title,
    status: 'active',
  };
}

function cleanKeyResults(items: KeyResult[]): KeyResult[] {
  return items
    .filter(item => item.title.trim())
    .map((item, index) => ({ ...item, code: `KR${index + 1}`, title: item.title.trim() }));
}

export default function GoalForm({ goal, goals = [], onSave, onUpdate, onDelete, onCancel, onAiCreate }: Props) {
  const [title, setTitle] = useState(goal?.title ?? '');
  const [description, setDescription] = useState(goal?.description ?? '');
  const [periodStart, setPeriodStart] = useState(goal?.periodStart ?? '');
  const [periodEnd, setPeriodEnd] = useState(goal?.periodEnd ?? '');
  const [keyResults, setKeyResults] = useState<KeyResult[]>(goal?.keyResults ?? []);
  const [showDetails, setShowDetails] = useState(false);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState('');
  const [editDescription, setEditDescription] = useState('');
  const [editPeriodStart, setEditPeriodStart] = useState('');
  const [editPeriodEnd, setEditPeriodEnd] = useState('');
  const [editKeyResults, setEditKeyResults] = useState<KeyResult[]>([]);

  const activeGoals = goals.filter(item => item.status === 'active');

  const resetNewGoal = () => {
    setTitle('');
    setDescription('');
    setPeriodStart('');
    setPeriodEnd('');
    setKeyResults([]);
    setShowDetails(false);
  };

  const submitNewGoal = (event: React.FormEvent) => {
    event.preventDefault();
    if (!title.trim()) return;
    onSave({
      title: title.trim(),
      description: description.trim() || undefined,
      periodStart: periodStart || undefined,
      periodEnd: periodEnd || undefined,
      keyResults: cleanKeyResults(keyResults),
    });
    resetNewGoal();
  };

  const startEditing = (item: Goal) => {
    setEditingId(item.id);
    setEditTitle(item.title);
    setEditDescription(item.description ?? '');
    setEditPeriodStart(item.periodStart ?? '');
    setEditPeriodEnd(item.periodEnd ?? '');
    setEditKeyResults(item.keyResults ?? []);
  };

  const saveEditing = (item: Goal) => {
    if (!editTitle.trim()) return;
    onUpdate?.(item.id, {
      title: editTitle.trim(),
      description: editDescription.trim() || undefined,
      deadline: item.deadline,
      periodStart: editPeriodStart || undefined,
      periodEnd: editPeriodEnd || undefined,
      keyResults: cleanKeyResults(editKeyResults),
    });
    setEditingId(null);
  };

  return (
    <section className="overflow-hidden rounded-[14px] border border-[#D9DDE4] bg-white shadow-[0_10px_30px_rgba(17,22,56,0.05)] dark:border-[#2A3039] dark:bg-[#14171C] dark:shadow-none">
      <header className="flex items-center justify-between border-b border-[#E6E8EC] px-4 py-3 dark:border-[#252A32]">
        <div>
          <h3 id="goal-management-title" className="text-[13px] font-semibold text-[#20242B] dark:text-[#E9ECF1]">目标与关键结果</h3>
          <p className="mt-0.5 text-[10px] text-[#7A818C] dark:text-[#8D96A3]">先写清想达成什么，再把结果拆成 KR</p>
        </div>
        <div className="flex items-center gap-1.5">
          {onAiCreate && (
            <button
              type="button"
              onClick={onAiCreate}
              className={`inline-flex h-8 items-center gap-1.5 rounded-[7px] bg-[#EEF1FC] px-2.5 text-[10px] font-medium text-brand hover:bg-[#E3E8FB] dark:bg-[#202639] dark:text-[#AEBBFF] dark:hover:bg-[#29314A] ${KEYBOARD_FOCUS}`}
            >
              <Sparkles size={12} />
              AI 创建 O / KR
            </button>
          )}
          <button type="button" onClick={onCancel} className={`rounded-[6px] p-1.5 text-[#7A818C] hover:bg-[#EEF0F3] hover:text-[#171A1F] dark:text-[#8D96A3] dark:hover:bg-[#242932] dark:hover:text-[#E9ECF1] ${KEYBOARD_FOCUS}`} aria-label="关闭目标管理">
            <X size={15} />
          </button>
        </div>
      </header>

      {activeGoals.length > 0 && (
        <div className="divide-y divide-[#ECEEF1] dark:divide-[#252A32]">
          {activeGoals.map(item => {
            const editing = editingId === item.id;
            return (
              <article key={item.id} className="px-4 py-3">
                {editing ? (
                  <div className="space-y-3">
                    <input
                      value={editTitle}
                      onChange={event => setEditTitle(event.target.value)}
                      className={`${INPUT_CLASS} h-9 text-[13px] font-semibold`}
                      placeholder="目标名称"
                      autoFocus
                    />
                    <textarea
                      value={editDescription}
                      onChange={event => setEditDescription(event.target.value)}
                      className={`${INPUT_CLASS} min-h-16 resize-y py-2 text-xs leading-5`}
                      placeholder="补充目标边界或成功标准（可选）"
                    />
                    <KeyResultEditor items={editKeyResults} onChange={setEditKeyResults} />
                    <div className="flex flex-wrap items-center gap-2">
                      <PeriodFields start={editPeriodStart} end={editPeriodEnd} onStart={setEditPeriodStart} onEnd={setEditPeriodEnd} />
                      <span className="flex-1" />
                      <button type="button" onClick={() => setEditingId(null)} className={`h-8 rounded-[7px] px-3 text-[11px] text-[#69717D] hover:bg-[#EEF0F3] dark:text-[#98A1AD] dark:hover:bg-[#242932] ${KEYBOARD_FOCUS}`}>取消</button>
                      <button type="button" onClick={() => saveEditing(item)} disabled={!editTitle.trim()} className={`inline-flex h-8 items-center gap-1 rounded-[7px] bg-[#20242B] px-3 text-[11px] font-medium text-white hover:bg-[#343A43] disabled:opacity-40 dark:bg-[#E9ECF1] dark:text-[#171A1F] ${KEYBOARD_FOCUS}`}>
                        <Check size={12} />保存
                      </button>
                    </div>
                  </div>
                ) : (
                  <div className="group flex items-start gap-3">
                    <span className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-[8px] bg-[#EEF1FC] text-[10px] font-bold text-brand dark:bg-[#202639] dark:text-[#9DB0FF]">O</span>
                    <div className="min-w-0 flex-1">
                      <h4 className="text-[13px] font-semibold text-[#30353D] dark:text-[#D9DEE6]">{item.title}</h4>
                      {item.description && <p className="mt-0.5 line-clamp-2 text-[11px] leading-5 text-[#69717D] dark:text-[#98A1AD]">{item.description}</p>}
                      <div className="mt-2 flex flex-wrap gap-1.5">
                        {(item.keyResults ?? []).map(result => (
                          <span key={result.id} className="rounded-[6px] bg-[#F3F4F6] px-2 py-1 text-[10px] text-[#4B535E] dark:bg-[#1E2229] dark:text-[#B3BAC5]">
                            <strong className="mr-1 font-mono text-[#69717D] dark:text-[#98A1AD]">{result.code}</strong>{result.title}
                          </span>
                        ))}
                        {(item.keyResults?.length ?? 0) === 0 && <span className="text-[10px] text-[#9AA1AC]">还没有 KR</span>}
                      </div>
                    </div>
                    <div className="flex shrink-0 items-center gap-0.5 opacity-100 sm:opacity-0 sm:transition-opacity sm:group-hover:opacity-100 sm:group-focus-within:opacity-100">
                      <button type="button" onClick={() => startEditing(item)} className={`rounded-[6px] p-1.5 text-[#7A818C] hover:bg-[#EEF0F3] hover:text-brand dark:text-[#8D96A3] dark:hover:bg-[#242932] dark:hover:text-[#9DB0FF] ${KEYBOARD_FOCUS}`} title="编辑目标">
                        <Edit3 size={13} />
                      </button>
                      <button type="button" onClick={() => onDelete?.(item.id)} className={`rounded-[6px] p-1.5 text-[#7A818C] hover:bg-[#FBEFEF] hover:text-[#A33D3D] dark:text-[#8D96A3] dark:hover:bg-[#2C2023] dark:hover:text-[#F19A9A] ${KEYBOARD_FOCUS}`} title="删除目标">
                        <Trash2 size={13} />
                      </button>
                    </div>
                  </div>
                )}
              </article>
            );
          })}
        </div>
      )}

      <form onSubmit={submitNewGoal} className="border-t border-[#E6E8EC] bg-[#FAFAFB] p-4 dark:border-[#252A32] dark:bg-[#181B21]">
        <div>
          <input
            value={title}
            onChange={event => setTitle(event.target.value)}
            placeholder="新目标名称"
            className={`${INPUT_CLASS} h-10 flex-1 bg-white text-[13px] dark:bg-[#14171C]`}
            autoFocus={activeGoals.length === 0}
          />
        </div>

        {keyResults.length > 0 && <div className="mt-3"><KeyResultEditor items={keyResults} onChange={setKeyResults} /></div>}

        <div className="mt-3 flex flex-wrap items-center gap-2">
          <button type="button" onClick={() => setShowDetails(value => !value)} className={`inline-flex h-8 items-center gap-1 rounded-[7px] px-2 text-[11px] text-[#69717D] hover:bg-[#ECEEF1] dark:text-[#98A1AD] dark:hover:bg-[#242932] ${KEYBOARD_FOCUS}`}>
            更多设置
            <ChevronDown size={12} className={`transition-transform ${showDetails ? 'rotate-180' : ''}`} />
          </button>
          <button type="button" onClick={() => setKeyResults(items => [...items, createKeyResult('', items.length)])} className={`inline-flex h-8 items-center gap-1 rounded-[7px] px-2 text-[11px] text-brand hover:bg-[#EEF1FC] dark:text-[#9DB0FF] dark:hover:bg-[#202639] ${KEYBOARD_FOCUS}`}>
            <Plus size={12} />添加 KR
          </button>
          <span className="flex-1" />
          <button type="submit" disabled={!title.trim()} className={`inline-flex h-9 items-center gap-1 rounded-[8px] bg-[#20242B] px-4 text-[11px] font-medium text-white hover:bg-[#343A43] disabled:cursor-not-allowed disabled:opacity-40 dark:bg-[#E9ECF1] dark:text-[#171A1F] ${KEYBOARD_FOCUS}`}>
            <Plus size={12} />创建目标
          </button>
        </div>

        {showDetails && (
          <div className="mt-3 space-y-2">
            <textarea value={description} onChange={event => setDescription(event.target.value)} className={`${INPUT_CLASS} min-h-16 resize-y py-2 text-xs leading-5 dark:bg-[#14171C]`} placeholder="目标边界或成功标准（可选）" />
            <PeriodFields start={periodStart} end={periodEnd} onStart={setPeriodStart} onEnd={setPeriodEnd} />
          </div>
        )}
      </form>
    </section>
  );
}

function KeyResultEditor({ items, onChange }: { items: KeyResult[]; onChange: (items: KeyResult[]) => void }) {
  return (
    <div className="space-y-1.5">
      {items.map((result, index) => (
        <div key={result.id} className="flex items-center gap-2">
          <span className="w-9 shrink-0 font-mono text-[10px] font-semibold text-[#69717D] dark:text-[#98A1AD]">KR{index + 1}</span>
          <input
            value={result.title}
            onChange={event => onChange(items.map(item => item.id === result.id ? { ...item, title: event.target.value } : item))}
            className={`${INPUT_CLASS} h-8 flex-1 bg-[#FAFAFB] text-[11px] dark:bg-[#181B21]`}
            placeholder="写一个可验证的结果"
            aria-label={`KR${index + 1}`}
          />
          <button type="button" onClick={() => onChange(items.filter(item => item.id !== result.id))} className={`rounded-[5px] p-1 text-[#8A919C] hover:bg-[#FBEFEF] hover:text-[#A33D3D] dark:hover:bg-[#2C2023] dark:hover:text-[#F19A9A] ${KEYBOARD_FOCUS}`} aria-label={`删除 KR${index + 1}`}>
            <X size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}

function PeriodFields({ start, end, onStart, onEnd }: { start: string; end: string; onStart: (value: string) => void; onEnd: (value: string) => void }) {
  return (
    <div className="grid min-w-[280px] flex-1 grid-cols-1 gap-2">
      <div className="min-w-0">
        <span className="mb-1 block text-[9px] font-medium text-[#8A919C] dark:text-[#7E8794]">开始日期</span>
        <DateInput value={start} onChange={onStart} className={`${INPUT_CLASS} h-8 min-w-0 text-[11px]`} ariaLabel="周期开始" />
      </div>
      <div className="min-w-0">
        <span className="mb-1 block text-[9px] font-medium text-[#8A919C] dark:text-[#7E8794]">结束日期</span>
        <DateInput value={end} onChange={onEnd} min={start || '1000-01-01'} className={`${INPUT_CLASS} h-8 min-w-0 text-[11px]`} ariaLabel="周期结束" />
      </div>
    </div>
  );
}
