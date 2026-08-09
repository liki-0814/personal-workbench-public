import { useEffect, useState, useCallback, useRef } from 'react';
import { X, Check, Trash2, Plus, ChevronDown, ExternalLink, Bot, FolderOpen } from 'lucide-react';
import { motion, AnimatePresence } from 'framer-motion';
import { generateId } from '@/core/utils/id';
import type { TodoItem, TodoSubTask, Goal } from '../../types';
import DateInput from './DateInput';

interface Props {
  todo: TodoItem | null;
  goals?: Goal[];
  onClose: () => void;
  onUpdate: (id: string, updates: Partial<Omit<TodoItem, 'id'>>) => void;
  onStartAi?: (todo: TodoItem) => void;
}

const PRIORITY_OPTIONS: { key: NonNullable<TodoItem['priority']>; label: string; color: string }[] = [
  { key: 'low',    label: '低', color: '#3b82f6' },
  { key: 'medium', label: '中', color: '#f59e0b' },
  { key: 'high',   label: '高', color: '#ef4444' },
];

const CONTROL_CLASS = 'rounded-[7px] border border-[#D9DDE4] bg-[#F7F8FA] px-3 py-2 text-[#171A1F] outline-none transition-[border-color,box-shadow,background-color] placeholder:text-[#9AA1AC] focus:border-[#B6BDC7] focus:bg-white focus:shadow-[0_0_0_3px_rgba(32,36,43,0.05)] dark:border-[#363D47] dark:bg-[#1A1E25] dark:text-[#E9ECF1] dark:focus:border-[#59616D] dark:focus:bg-[#14171C] dark:focus:shadow-[0_0_0_3px_rgba(233,236,241,0.05)]';

export default function TaskDrawer({ todo, goals, onClose, onUpdate, onStartAi }: Props) {
  const [title, setTitle] = useState('');
  const [priority, setPriority] = useState<TodoItem['priority']>('medium');
  const [dueDate, setDueDate] = useState('');
  const [focusTimerMode, setFocusTimerMode] = useState<'countdown' | 'countup'>('countdown');
  const [focusMinutes, setFocusMinutes] = useState('25');
  const [notesText, setNotesText] = useState('');
  const [deliverableUrl, setDeliverableUrl] = useState('');
  const [subTasks, setSubTasks] = useState<TodoSubTask[]>([]);
  const [goalId, setGoalId] = useState<string | undefined>(undefined);
  const [keyResultId, setKeyResultId] = useState<string | undefined>(undefined);
  const [goalMenuOpen, setGoalMenuOpen] = useState(false);
  const [keyResultMenuOpen, setKeyResultMenuOpen] = useState(false);
  const goalMenuRef = useRef<HTMLDivElement>(null);
  const keyResultMenuRef = useRef<HTMLDivElement>(null);
  const titleInputRef = useRef<HTMLInputElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);

  // Sync from selected todo
  useEffect(() => {
    if (!todo) return;
    setTitle(todo.title);
    setPriority(todo.priority || 'medium');
    setDueDate(todo.dueDate || '');
    setFocusTimerMode(todo.focusTimerMode || 'countdown');
    setFocusMinutes(String(todo.focusMinutes || todo.estimatedMinutes || 25));
    setNotesText(todo.notes || '');
    setDeliverableUrl(todo.deliverableUrl || '');
    setSubTasks(todo.subTasks || []);
    setGoalId(todo.goalId);
    setKeyResultId(todo.keyResultId);
  }, [todo?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const flush = useCallback(() => {
    if (!todo) return null;
    const patch: Partial<Omit<TodoItem, 'id'>> = {
      title: title.trim() || todo.title,
      priority,
      dueDate: dueDate || undefined,
      focusTimerMode,
      focusMinutes: focusTimerMode === 'countdown'
        ? Math.max(5, Math.min(120, Number.parseInt(focusMinutes, 10) || 25))
        : undefined,
      notes: notesText.trim() || undefined,
      deliverableUrl: deliverableUrl.trim() || undefined,
      subTasks: subTasks.length > 0 ? subTasks : undefined,
      goalId: goalId || undefined,
      keyResultId: keyResultId || undefined,
    };
    onUpdate(todo.id, patch);
    return { ...todo, ...patch };
  }, [todo, title, priority, dueDate, focusTimerMode, focusMinutes, notesText, deliverableUrl, subTasks, goalId, keyResultId, onUpdate]);

  const closeDrawer = useCallback(() => {
    flush();
    onClose();
  }, [flush, onClose]);

  useEffect(() => {
    if (!todo) {
      if (returnFocusRef.current?.isConnected) returnFocusRef.current.focus();
      returnFocusRef.current = null;
      return;
    }
    const activeElement = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    if (activeElement && !titleInputRef.current?.closest('aside')?.contains(activeElement)) {
      returnFocusRef.current = activeElement;
    }
    const timer = window.setTimeout(() => titleInputRef.current?.focus(), 0);
    return () => window.clearTimeout(timer);
  }, [todo?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!goalMenuOpen && !keyResultMenuOpen) return;
    const closeOnOutsideClick = (event: PointerEvent) => {
      if (!goalMenuRef.current?.contains(event.target as Node) && !keyResultMenuRef.current?.contains(event.target as Node)) {
        setGoalMenuOpen(false);
        setKeyResultMenuOpen(false);
      }
    };
    window.addEventListener('pointerdown', closeOnOutsideClick);
    return () => window.removeEventListener('pointerdown', closeOnOutsideClick);
  }, [goalMenuOpen, keyResultMenuOpen]);

  // 下拉菜单优先消费 Esc；否则关闭非模态抽屉。
  useEffect(() => {
    if (!todo) return;
    const h = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      if (goalMenuOpen || keyResultMenuOpen) {
        setGoalMenuOpen(false);
        setKeyResultMenuOpen(false);
      } else {
        closeDrawer();
      }
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [closeDrawer, todo, goalMenuOpen, keyResultMenuOpen]);

  const addSub = () => setSubTasks(prev => [...prev, { id: generateId(), title: '', completed: false }]);
  const updSub = (id: string, upd: Partial<TodoSubTask>) =>
    setSubTasks(prev => prev.map(s => s.id === id ? { ...s, ...upd } : s));
  const rmSub = (id: string) => setSubTasks(prev => prev.filter(s => s.id !== id));

  return (
    <AnimatePresence>
      {todo && (
          <motion.aside
            initial={{ x: '100%' }}
            animate={{ x: 0 }}
            exit={{ x: '100%' }}
            transition={{ duration: 0.22, ease: [0.32, 0.72, 0, 1] }}
            aria-labelledby="task-drawer-title"
            className="absolute bottom-0 right-0 top-0 z-50 flex w-[clamp(420px,34vw,520px)] max-w-[92%] flex-col overflow-hidden rounded-l-[18px] border-l border-[#D9DDE4] bg-white shadow-[-18px_0_48px_rgba(17,22,56,0.14)] dark:border-[#2A3039] dark:bg-[#14171C] dark:shadow-[-18px_0_52px_rgba(0,0,0,0.4)]"
            onClick={e => e.stopPropagation()}
          >
            {/* Header */}
            <div className="flex items-center gap-2 border-b border-[#D9DDE4] px-4 py-3 dark:border-[#2A3039]">
              <input
                ref={titleInputRef}
                id="task-drawer-title"
                aria-label="任务标题"
                value={title}
                onChange={e => setTitle(e.target.value)}
                onBlur={flush}
                placeholder="任务标题"
                className="min-w-0 flex-1 rounded-[6px] border-0 bg-transparent px-1 py-1 text-[17px] font-semibold tracking-[-0.02em] text-[#171A1F] outline-none transition-colors hover:bg-[#F7F8FA] focus:bg-[#F7F8FA] dark:text-[#E9ECF1] dark:hover:bg-[#1A1E25] dark:focus:bg-[#1A1E25]"
              />
              {onStartAi && (
                <button type="button" onClick={() => { const updated = flush(); if (updated) onStartAi(updated); }} className="inline-flex h-8 items-center gap-1.5 rounded-[7px] border border-[#D7DCE5] bg-[#F7F8FA] px-2.5 text-[11px] font-medium text-brand transition-colors hover:border-[#BFC8E8] hover:bg-[#EEF1FC] dark:border-[#363D47] dark:bg-[#1A1E25] dark:text-[#9DB0FF] dark:hover:bg-[#202639]" title="让 pwcli 协作执行">
                  <Bot size={13} />AI 协作
                </button>
              )}
              <button
                onClick={closeDrawer}
                className="rounded-[6px] p-2 text-[#7A818C] transition-colors hover:bg-[#EEF0F3] hover:text-[#171A1F] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 dark:text-[#8D96A3] dark:hover:bg-[#1A1E25] dark:hover:text-[#E9ECF1]"
                aria-label="关闭任务详情"
                title="关闭 (Esc)"
              >
                <X size={16} />
              </button>
            </div>

            {/* Body */}
            <div className="flex-1 space-y-4 overflow-y-auto px-4 py-3.5">
              {/* 优先级 */}
              <Field label="优先级">
                <div className="flex gap-1.5">
                  {PRIORITY_OPTIONS.map(p => {
                    const active = priority === p.key;
                    return (
                      <button
                        key={p.key}
                        onClick={() => { setPriority(p.key); }}
                        onBlur={flush}
                        className={`flex flex-1 items-center justify-center gap-1.5 rounded-[7px] border px-3 py-2 text-xs font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 ${
                          active
                            ? 'border-transparent bg-[#EEF1FC] text-[#2448C5] dark:bg-[#202639] dark:text-[#E9ECF1]'
                            : 'border-[#D9DDE4] text-[#69717D] hover:bg-[#EEF0F3] dark:border-[#363D47] dark:text-[#98A1AD] dark:hover:bg-[#1A1E25]'
                        }`}
                      >
                        <span className="w-2 h-2 rounded-full" style={{ background: p.color }} />
                        {p.label}
                      </button>
                    );
                  })}
                </div>
              </Field>

              {/* 截止日期 */}
              <Field label="截止日期">
                <DateInput
                  value={dueDate}
                  onChange={value => {
                    setDueDate(value);
                    if (todo) onUpdate(todo.id, { dueDate: value || undefined });
                  }}
                  onBlur={flush}
                  className={`${CONTROL_CLASS} w-full text-sm`}
                  ariaLabel="截止日期"
                  quickOptions
                />
              </Field>

              <Field label="交付产物链接">
                <div className="relative">
                  <ExternalLink size={14} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-[#8A919C] dark:text-[#7E8794]" />
                  <input
                    type="url"
                    value={deliverableUrl}
                    onChange={event => setDeliverableUrl(event.target.value)}
                    onBlur={flush}
                    placeholder="https://..."
                    className={`${CONTROL_CLASS} w-full pl-9 text-sm`}
                  />
                </div>
              </Field>

              {todo.executionWorkspace && (
                <Field label="任务执行项目">
                  <div className="flex items-center gap-2 rounded-[8px] border border-[#D9DDE4] bg-[#F7F8FA] px-3 py-2 text-xs text-[#4B5563] dark:border-[#363D47] dark:bg-[#1A1E25] dark:text-[#AAB2BE]">
                    <FolderOpen size={13} className="shrink-0" />
                    <span className="min-w-0 flex-1 truncate font-mono" title={todo.executionWorkspace.path}>{todo.executionWorkspace.path}</span>
                    <button type="button" onClick={() => onUpdate(todo.id, { executionWorkspace: undefined })} className="text-[10px] text-[#69717D] hover:text-[#A33D3D]">解除</button>
                  </div>
                </Field>
              )}

              {/* 关联目标 */}
              {goals && goals.length > 0 && (
                <Field label="关联 O">
                  <div ref={goalMenuRef} className="relative">
                    <button
                      type="button"
                      onClick={() => setGoalMenuOpen(open => !open)}
                      className={`${CONTROL_CLASS} flex w-full items-center justify-between text-sm`}
                      aria-haspopup="listbox"
                      aria-expanded={goalMenuOpen}
                    >
                      <span className="truncate">
                        {(() => {
                          const selectedGoal = goals.find(goal => goal.id === goalId);
                          if (!selectedGoal) return '无';
                          return `${selectedGoal.emoji ? `${selectedGoal.emoji} ` : ''}${selectedGoal.title}`;
                        })()}
                      </span>
                      <ChevronDown
                        size={15}
                        className={`ml-2 shrink-0 transition-transform ${goalMenuOpen ? 'rotate-180' : ''}`}
                      />
                    </button>
                    {goalMenuOpen && (
                      <div
                        role="listbox"
                        aria-label="关联目标"
                        className="absolute left-0 right-0 top-full z-20 mt-2 max-h-56 overflow-y-auto rounded-[12px] border border-[#D9DDE4] bg-white p-1.5 shadow-[0_16px_40px_rgba(17,22,56,0.16)] dark:border-[#363D47] dark:bg-[#1A1E25]"
                      >
                        {[{ id: '', title: '无', emoji: '' }, ...goals.filter(goal => goal.status === 'active')].map(goal => {
                          const selected = (goalId || '') === goal.id;
                          return (
                            <button
                              key={goal.id || 'none'}
                              type="button"
                              role="option"
                              aria-selected={selected}
                              onClick={() => {
                                const nextGoalId = goal.id || undefined;
                                setGoalId(nextGoalId);
                                setKeyResultId(undefined);
                                setGoalMenuOpen(false);
                                if (todo) onUpdate(todo.id, { goalId: nextGoalId, keyResultId: undefined });
                              }}
                              className={`flex w-full items-center gap-2 rounded-[8px] px-3 py-2 text-left text-sm transition-colors ${
                                selected
                                  ? 'bg-[#EEF1FC] font-medium text-[#2448C5] dark:bg-[#202639] dark:text-[#E9ECF1]'
                                  : 'text-[#30353D] hover:bg-[#F1F2F4] dark:text-[#D9DEE6] dark:hover:bg-[#242A32]'
                              }`}
                            >
                              <span className="min-w-0 flex-1 truncate">
                                {goal.emoji ? `${goal.emoji} ` : ''}{goal.title}
                              </span>
                              {selected && <Check size={14} className="shrink-0" />}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                </Field>
              )}

              {goalId && (() => {
                const selectedGoal = goals?.find(goal => goal.id === goalId);
                const keyResults = selectedGoal?.keyResults ?? [];
                if (keyResults.length === 0) return null;
                return (
                  <Field label="归属 KR">
                    <div ref={keyResultMenuRef} className="relative">
                      <button
                        type="button"
                        onClick={() => setKeyResultMenuOpen(open => !open)}
                        className={`${CONTROL_CLASS} flex w-full items-center justify-between text-sm`}
                        aria-haspopup="listbox"
                        aria-expanded={keyResultMenuOpen}
                      >
                        <span className="truncate">
                          {keyResults.find(result => result.id === keyResultId)?.code ?? '未归属 KR'}
                        </span>
                        <ChevronDown size={15} className={`ml-2 shrink-0 transition-transform ${keyResultMenuOpen ? 'rotate-180' : ''}`} />
                      </button>
                      {keyResultMenuOpen && (
                        <div role="listbox" aria-label="归属 KR" className="absolute left-0 right-0 top-full z-20 mt-2 max-h-64 overflow-y-auto rounded-[12px] border border-[#D9DDE4] bg-white p-1.5 shadow-[0_16px_40px_rgba(17,22,56,0.16)] dark:border-[#363D47] dark:bg-[#1A1E25]">
                          {[undefined, ...keyResults].map(result => {
                            const nextId = result?.id;
                            const selected = keyResultId === nextId;
                            return (
                              <button
                                key={nextId ?? 'none'}
                                type="button"
                                role="option"
                                aria-selected={selected}
                                onClick={() => {
                                  setKeyResultId(nextId);
                                  setKeyResultMenuOpen(false);
                                  if (todo) onUpdate(todo.id, { goalId, keyResultId: nextId });
                                }}
                                className={`flex w-full items-start gap-2 rounded-[8px] px-3 py-2 text-left text-xs transition-colors ${selected ? 'bg-[#EEF1FC] text-[#2448C5] dark:bg-[#202639] dark:text-[#E9ECF1]' : 'text-[#30353D] hover:bg-[#F1F2F4] dark:text-[#D9DEE6] dark:hover:bg-[#242A32]'}`}
                              >
                                <strong className="w-8 shrink-0 font-mono">{result?.code ?? '无'}</strong>
                                <span className="min-w-0 flex-1 leading-5">{result?.title ?? '暂不归属到 KR'}</span>
                                {selected && <Check size={14} className="mt-0.5 shrink-0" />}
                              </button>
                            );
                          })}
                        </div>
                      )}
                    </div>
                  </Field>
                );
              })()}

              <Field label="协作成长">
                <button
                  type="button"
                  role="switch"
                  aria-checked={todo?.learningIntent === true}
                  onClick={() => todo && onUpdate(todo.id, { learningIntent: !todo.learningIntent })}
                  className={`flex w-full items-center justify-between rounded-[8px] border px-3 py-2.5 text-left transition-colors ${
                    todo?.learningIntent
                      ? 'border-transparent bg-[#EEF1FC] text-[#2448C5] dark:bg-[#202639] dark:text-[#E9ECF1]'
                      : 'border-[#D9DDE4] text-[#69717D] hover:bg-[#EEF0F3] dark:border-[#363D47] dark:text-[#98A1AD] dark:hover:bg-[#1A1E25]'
                  }`}
                >
                  <span>
                    <strong className="block text-xs">我想学会</strong>
                    <small className="mt-0.5 block text-[10px] opacity-75">只在一个关键判断点邀请我参与</small>
                  </span>
                  <span className={`h-5 w-9 rounded-full p-0.5 transition-colors ${todo?.learningIntent ? 'bg-brand' : 'bg-[#C9CDD4] dark:bg-[#4A515C]'}`}>
                    <i className={`block h-4 w-4 rounded-full bg-white transition-transform ${todo?.learningIntent ? 'translate-x-4' : ''}`} />
                  </span>
                </button>
              </Field>

              <Field label="专注计时">
                <div className="space-y-2">
                  <div className="grid grid-cols-2 gap-1.5">
                    <button
                      onClick={() => setFocusTimerMode('countdown')}
                      onBlur={flush}
                      className={`rounded-[6px] border px-3 py-2 text-xs font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 ${
                        focusTimerMode === 'countdown'
                          ? 'border-transparent bg-[#EEF1FC] text-[#2448C5] dark:bg-[#202639] dark:text-[#E9ECF1]'
                          : 'border-[#D9DDE4] text-[#69717D] hover:bg-[#EEF0F3] dark:border-[#363D47] dark:text-[#98A1AD] dark:hover:bg-[#1A1E25]'
                      }`}
                    >
                      倒计时
                    </button>
                    <button
                      onClick={() => setFocusTimerMode('countup')}
                      onBlur={flush}
                      className={`rounded-[6px] border px-3 py-2 text-xs font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 ${
                        focusTimerMode === 'countup'
                          ? 'border-transparent bg-[#EEF1FC] text-[#2448C5] dark:bg-[#202639] dark:text-[#E9ECF1]'
                          : 'border-[#D9DDE4] text-[#69717D] hover:bg-[#EEF0F3] dark:border-[#363D47] dark:text-[#98A1AD] dark:hover:bg-[#1A1E25]'
                      }`}
                    >
                      正计时
                    </button>
                  </div>
                  {focusTimerMode === 'countdown' && (
                    <div className="flex items-center gap-2">
                      <input
                        type="number"
                        min={5}
                        max={120}
                        value={focusMinutes}
                        onChange={e => setFocusMinutes(e.target.value)}
                        onBlur={flush}
                        className={`${CONTROL_CLASS} w-full text-sm`}
                      />
                      <span className="w-10 flex-shrink-0 text-xs text-[#7A818C] dark:text-[#8D96A3]">分钟</span>
                    </div>
                  )}
                </div>
              </Field>

              {/* 子任务 */}
              <Field label={`步骤 (${subTasks.filter(s => s.completed).length}/${subTasks.length})`}>
                <div className="space-y-1.5">
                  {subTasks.map(st => (
                    <div key={st.id} className="flex items-center gap-2 group">
                      <button
                        onClick={() => { updSub(st.id, { completed: !st.completed }); flush(); }}
                        className={`flex h-4 w-4 flex-shrink-0 items-center justify-center rounded-[5px] border transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 ${
                          st.completed
                            ? 'border-[#4F7A61] bg-[#4F7A61] text-white'
                            : 'border-[#B9BFC8] hover:border-brand dark:border-[#4B535E] dark:hover:border-[#7B96FF]'
                        }`}
                      >
                        {st.completed && <Check size={9} strokeWidth={3} />}
                      </button>
                      <input
                        value={st.title}
                        onChange={e => updSub(st.id, { title: e.target.value })}
                        onBlur={flush}
                        placeholder="步骤..."
                        className={`${CONTROL_CLASS} flex-1 py-1.5 text-xs ${st.completed ? 'line-through opacity-60' : ''}`}
                      />
                      <button
                        onClick={() => { rmSub(st.id); flush(); }}
                        className="rounded-[4px] p-1 text-[#7A818C] opacity-0 transition-colors hover:bg-[#FBEFEF] hover:text-[#A33D3D] group-hover:opacity-100 dark:text-[#8D96A3] dark:hover:bg-[#2C2023] dark:hover:text-[#F19A9A]"
                      >
                        <Trash2 size={11} />
                      </button>
                    </div>
                  ))}
                  <button
                    onClick={addSub}
                    className="inline-flex w-full items-center justify-center gap-1 rounded-[6px] border border-dashed border-[#BFC5CE] py-1.5 text-xs text-[#69717D] transition-colors hover:border-[#7B96FF] hover:text-brand dark:border-[#4B535E] dark:text-[#98A1AD] dark:hover:border-[#7B96FF] dark:hover:text-[#9DB0FF]"
                  >
                    <Plus size={11} /> 添加步骤
                  </button>
                </div>
              </Field>

              {/* 备注 */}
              <Field label="备注">
                <textarea
                  value={notesText}
                  onChange={e => setNotesText(e.target.value)}
                  onBlur={flush}
                  rows={4}
                  placeholder="添加备注..."
                  className={`${CONTROL_CLASS} w-full resize-none text-sm`}
                />
              </Field>
            </div>
          </motion.aside>
      )}
    </AnimatePresence>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="mb-1.5 text-[11px] font-medium text-[#69717D] dark:text-[#98A1AD]">{label}</div>
      {children}
    </div>
  );
}
