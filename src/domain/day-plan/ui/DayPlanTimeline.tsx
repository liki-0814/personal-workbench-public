import { useEffect, useMemo, useRef, useState } from 'react';
import { Check, Clock3, GripVertical, Plus, Trash2 } from 'lucide-react';
import { showToast } from '@/shell';
import { SelectField } from '@/shell';
import type { TodoItem } from '@/domain/todo';
import type { DayPlanItem } from '../types';
import {
  clampStart,
  clampDuration,
  DAY_END_MINUTE,
  DAY_START_MINUTE,
  formatMinute,
  minuteOfDay,
  parseTime,
  snapMinute,
} from '../utils';

interface Props {
  plans: DayPlanItem[];
  todos?: TodoItem[];
  onAdd: (title: string, startMinute: number, durationMinutes?: number, sourceTodoId?: string) => void;
  onUpdate: (id: string, patch: Partial<Pick<DayPlanItem, 'title' | 'startMinute' | 'durationMinutes'>>) => void;
  onRemove: (id: string) => DayPlanItem | undefined;
  onRestore: (item: DayPlanItem) => void;
}

const RANGE = DAY_END_MINUTE - DAY_START_MINUTE;
const HOURS = Array.from({ length: 17 }, (_, index) => 8 + index);

type DragState = {
  id: string;
  mode: 'move' | 'resize';
  pointerStart: number;
  originalStart: number;
  originalDuration: number;
  draftStart: number;
  draftDuration: number;
};

export default function DayPlanTimeline({ plans, todos = [], onAdd, onUpdate, onRemove, onRestore }: Props) {
  const [title, setTitle] = useState('');
  const [time, setTime] = useState(() => formatMinute(clampStart(minuteOfDay())));
  const [duration, setDuration] = useState(30);
  const [durationInput, setDurationInput] = useState('0.5');
  const [sourceTodoId, setSourceTodoId] = useState('');
  const [nowMinute, setNowMinute] = useState(minuteOfDay);
  const [drag, setDrag] = useState<DragState | null>(null);
  const timelineRef = useRef<HTMLDivElement>(null);
  const inProgressTodos = useMemo(
    () => todos.filter(todo => !todo.completed && todo.status === 'in_progress'),
    [todos],
  );

  useEffect(() => {
    const timer = window.setInterval(() => setNowMinute(minuteOfDay()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (sourceTodoId && !inProgressTodos.some(todo => todo.id === sourceTodoId)) {
      setSourceTodoId('');
    }
  }, [inProgressTodos, sourceTodoId]);

  useEffect(() => {
    if (!drag) return;
    const handleMove = (event: PointerEvent) => {
      const rect = timelineRef.current?.getBoundingClientRect();
      if (!rect) return;
      const delta = snapMinute((event.clientY - drag.pointerStart) / rect.height * RANGE);
      setDrag(current => {
        if (!current) return null;
        if (current.mode === 'move') return { ...current, draftStart: clampStart(current.originalStart + delta, current.originalDuration) };
        return { ...current, draftDuration: clampDuration(current.originalDuration + delta, current.originalStart) };
      });
    };
    const handleUp = () => setDrag(current => {
      if (current) onUpdate(current.id, { startMinute: current.draftStart, durationMinutes: current.draftDuration });
      return null;
    });
    window.addEventListener('pointermove', handleMove);
    window.addEventListener('pointerup', handleUp, { once: true });
    return () => {
      window.removeEventListener('pointermove', handleMove);
      window.removeEventListener('pointerup', handleUp);
    };
  }, [drag, onUpdate]);

  const active = useMemo(
    () => plans.find(plan => nowMinute >= plan.startMinute && nowMinute < plan.startMinute + plan.durationMinutes),
    [nowMinute, plans],
  );
  const next = useMemo(() => plans.find(plan => plan.startMinute > nowMinute), [nowMinute, plans]);
  const totalMinutes = plans.reduce((sum, plan) => sum + plan.durationMinutes, 0);

  const add = () => {
    const start = parseTime(time);
    if (!title.trim() || start == null) return;
    const finalDuration = clampDuration(duration, start);
    onAdd(title, start, finalDuration, sourceTodoId || undefined);
    setTitle('');
    setDuration(finalDuration);
    setDurationInput(String(finalDuration / 60));
    setTime(formatMinute(start + finalDuration >= DAY_END_MINUTE ? DAY_START_MINUTE : clampStart(start + finalDuration)));
  };

  const finish = (id: string) => {
    const removed = onRemove(id);
    if (!removed) return;
    showToast({
      message: `已完成：${removed.title}`,
      type: 'success',
      duration: 5000,
      action: { label: '撤销', onClick: () => onRestore(removed) },
    });
  };

  const remove = (id: string) => {
    const removed = onRemove(id);
    if (!removed) return;
    showToast({
      message: '已删除日内计划',
      type: 'info',
      duration: 5000,
      action: { label: '撤销', onClick: () => onRestore(removed) },
    });
  };

  const currentTop = Math.max(0, Math.min(100, (nowMinute - DAY_START_MINUTE) / RANGE * 100));

  return (
    <section className="day-plan-shell min-h-0 pb-8">
      <div className="mb-5 grid gap-2 sm:grid-cols-3">
        <div className="day-plan-summary day-plan-summary-active sm:col-span-2">
          <span>现在</span>
          <strong>{active?.title || '留白时间'}</strong>
          <small>{active ? `${formatMinute(active.startMinute)}–${formatMinute(active.startMinute + active.durationMinutes)}` : next ? `${formatMinute(next.startMinute)} 开始 ${next.title}` : '今天没有后续安排'}</small>
        </div>
        <div className="day-plan-summary">
          <span>今日安排</span>
          <strong>{plans.length} 段 · {Math.floor(totalMinutes / 60)}h {totalMinutes % 60}m</strong>
          <small>{next ? `下一段 ${formatMinute(next.startMinute)}` : '保持轻松'}</small>
        </div>
      </div>

      <form
        className="day-plan-quick-add"
        onSubmit={(event) => { event.preventDefault(); add(); }}
      >
        <label>
          <input
            type="time"
            min={formatMinute(DAY_START_MINUTE)}
            max={formatMinute(DAY_END_MINUTE - 30)}
            step="300"
            value={time}
            onChange={event => {
              setTime(event.target.value);
              const start = parseTime(event.target.value);
              if (start != null) {
                const nextDuration = clampDuration(duration, start);
                setDuration(nextDuration);
                setDurationInput(String(nextDuration / 60));
              }
            }}
            aria-label="开始时间"
          />
        </label>
        <label className="day-plan-duration">
          <input
            type="text"
            inputMode="decimal"
            value={durationInput}
            onChange={event => {
              const raw = event.target.value;
              if (raw === '' || /^\d+\.$/.test(raw)) {
                setDurationInput(raw);
                return;
              }
              if (!/^\d+(?:\.[05])?$/.test(raw)) return;
              const start = parseTime(time);
              if (start == null) return;
              const nextDuration = clampDuration(Number(raw) * 60, start);
              setDuration(nextDuration);
              setDurationInput(String(nextDuration / 60));
            }}
            onBlur={() => setDurationInput(String(duration / 60))}
            aria-label="持续小时数"
          />
        </label>
        <input
          value={title}
          onChange={event => setTitle(event.target.value)}
          placeholder="接下来专注什么？"
          aria-label="计划内容"
        />
        <button type="submit" disabled={!title.trim()}><Plus size={15} />排入今天</button>
      </form>
      {inProgressTodos.length > 0 && (
        <div className="mt-2 max-w-[360px]">
          <SelectField
            value={sourceTodoId}
            onValueChange={setSourceTodoId}
            options={[
              { value: '', label: '临时安排（不绑定任务）' },
              ...inProgressTodos.map(todo => ({
                value: todo.id,
                label: `进行中 · ${todo.title}`,
              })),
            ]}
            density="compact"
            ariaLabel="绑定任务"
          />
        </div>
      )}

      <div className="mt-5 flex items-center justify-between px-1">
        <div>
          <h3 className="text-[15px] font-semibold tracking-[-0.015em] text-[#20242B] dark:text-[#E9ECF1]">今天的节奏</h3>
          <p className="mt-0.5 text-[11px] font-medium text-[#8A919C] dark:text-[#7E8794]">开始时间 5 分钟对齐，持续时间按 0.5 小时调整</p>
        </div>
        <span className="font-mono text-[11px] tabular-nums text-[#7A818C] dark:text-[#8D96A3]">08:00 — 24:00</span>
      </div>

      <div className="day-plan-canvas mt-3">
        <div className="day-plan-hours" aria-hidden="true">
          {HOURS.map(hour => <span key={hour}>{String(hour).padStart(2, '0')}</span>)}
        </div>
        <div
          ref={timelineRef}
          className={`day-plan-track ${drag ? 'select-none' : ''}`}
          onDoubleClick={(event) => {
            if (event.target !== event.currentTarget) return;
            const rect = event.currentTarget.getBoundingClientRect();
            const start = clampStart(DAY_START_MINUTE + (event.clientY - rect.top) / rect.height * RANGE);
            setTime(formatMinute(start));
          }}
        >
          {nowMinute >= DAY_START_MINUTE && nowMinute <= DAY_END_MINUTE && (
            <div className="day-plan-now" style={{ top: `${currentTop}%` }}>
              <span>{formatMinute(nowMinute)}</span>
            </div>
          )}
          {plans.map((plan, index) => {
            const draft = drag?.id === plan.id ? drag : null;
            const displayStart = draft?.draftStart ?? plan.startMinute;
            const displayDuration = draft?.draftDuration ?? plan.durationMinutes;
            const top = (displayStart - DAY_START_MINUTE) / RANGE * 100;
            const height = displayDuration / RANGE * 100;
            const isPast = plan.startMinute + plan.durationMinutes <= nowMinute;
            const isActive = active?.id === plan.id;
            return (
              <article
                key={plan.id}
                className={`day-plan-block day-plan-color-${index % 4} ${isActive ? 'is-active' : ''} ${isPast ? 'is-past' : ''}`}
                style={{ top: `${top}%`, height: `${Math.max(height, 2.8)}%` }}
                onPointerDown={event => {
                  if ((event.target as HTMLElement).closest('button,input')) return;
                  event.preventDefault();
                  setDrag({ id: plan.id, mode: 'move', pointerStart: event.clientY, originalStart: plan.startMinute, originalDuration: plan.durationMinutes, draftStart: plan.startMinute, draftDuration: plan.durationMinutes });
                }}
              >
                <GripVertical className="day-plan-grip" size={13} />
                <div className="min-w-0 flex-1">
                  <div className="flex items-baseline gap-2">
                    <time>{formatMinute(plan.startMinute)}</time>
                    <input
                      value={plan.title}
                      onChange={event => onUpdate(plan.id, { title: event.target.value })}
                      aria-label="计划标题"
                    />
                  </div>
                  {plan.durationMinutes >= 30 && <small>{plan.durationMinutes / 60} 小时</small>}
                </div>
                <div className="day-plan-actions">
                  <button type="button" onClick={() => finish(plan.id)} title="完成并移除"><Check size={14} /></button>
                  <button type="button" onClick={() => remove(plan.id)} title="删除"><Trash2 size={13} /></button>
                </div>
                <button
                  type="button"
                  className="day-plan-resize"
                  aria-label="调整计划时长"
                  onPointerDown={event => {
                    event.preventDefault();
                    event.stopPropagation();
                    setDrag({ id: plan.id, mode: 'resize', pointerStart: event.clientY, originalStart: plan.startMinute, originalDuration: plan.durationMinutes, draftStart: plan.startMinute, draftDuration: plan.durationMinutes });
                  }}
                />
              </article>
            );
          })}
          {plans.length === 0 && (
            <button type="button" className="day-plan-empty" onClick={() => setTime(formatMinute(clampStart(minuteOfDay())))}>
              <Clock3 size={18} />
              <strong>从当前时间排第一段</strong>
              <span>写下要做的事，默认安排 30 分钟</span>
            </button>
          )}
        </div>
      </div>
    </section>
  );
}
