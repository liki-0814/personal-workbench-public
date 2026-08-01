import { useEffect, useRef, useState } from 'react';
import { ArrowUp, Check, Loader2, Sparkles, Target, X } from 'lucide-react';
import { generateId } from '@/core/utils/id';
import { showToast } from '@/shell';
import { parseWorkbenchInput, type WorkbenchAiDraft } from '../../ai/parseWorkbenchInput';
import { shouldSubmitOnEnter } from '../../utils/keyboard';
import { resolveAiTarget } from '../../utils/resolveAiTarget';
import type { Goal, KeyResult, TodoItem } from '../../types';

interface ObjectiveDraft {
  intent: 'objective';
  title: string;
  description: string;
  keyResults: KeyResult[];
}

interface KeyResultDraft {
  intent: 'key_result';
  goalId: string;
  keyResults: KeyResult[];
}

type ConfirmableDraft = ObjectiveDraft | KeyResultDraft;

interface Props {
  todayDoneCount: number;
  todayTotalCount: number;
  weekTotalCount: number;
  pomodoroTodayMinutes: number;
  onAdd: (
    title: string,
    type: TodoItem['type'],
    notes?: string,
    priority?: TodoItem['priority'],
    dueDate?: string,
    subTasks?: TodoItem['subTasks'],
    goalId?: string,
    keyResultId?: string,
  ) => void;
  goals?: Goal[];
  onCreateGoal?: (data: { title: string; description?: string; keyResults: KeyResult[] }) => void;
  onAppendKeyResults?: (goalId: string, keyResults: KeyResult[]) => void;
  onManageGoals?: () => void;
  aiFocusRequest?: number;
}

const KEYBOARD_FOCUS = 'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 focus-visible:ring-offset-2 dark:focus-visible:ring-offset-[#14171C]';
const INPUT_FOCUS = 'focus:border-[#B6BDC7] focus:bg-white focus:shadow-[0_0_0_3px_rgba(32,36,43,0.05)] dark:focus:border-[#59616D] dark:focus:bg-[#14171C] dark:focus:shadow-[0_0_0_3px_rgba(233,236,241,0.05)]';

function createKeyResult(title: string, index: number): KeyResult {
  return { id: `kr-${generateId()}`, code: `KR${index + 1}`, title, status: 'active' };
}

export default function TaskHero({
  todayDoneCount,
  todayTotalCount,
  weekTotalCount,
  pomodoroTodayMinutes,
  onAdd,
  goals = [],
  onCreateGoal,
  onAppendKeyResults,
  onManageGoals,
  aiFocusRequest = 0,
}: Props) {
  const [value, setValue] = useState('');
  const [loading, setLoading] = useState(false);
  const [draft, setDraft] = useState<ConfirmableDraft | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const keyHandler = (event: KeyboardEvent) => {
      if (event.ctrlKey && !event.metaKey && event.key.toLowerCase() === 'n') {
        event.preventDefault();
        inputRef.current?.focus();
      }
    };
    window.addEventListener('keydown', keyHandler);
    return () => window.removeEventListener('keydown', keyHandler);
  }, []);

  useEffect(() => {
    if (aiFocusRequest > 0) inputRef.current?.focus();
  }, [aiFocusRequest]);

  const saveTask = (result: Extract<WorkbenchAiDraft, { intent: 'task' }>) => {
    const subTasks = result.subTasks.map(item => ({ id: generateId(), title: item.title, completed: false }));
    const resolved = resolveAiTarget(goals, result.objectiveTitle, result.keyResultTitle);
    onAdd(
      result.title,
      'today',
      result.notes,
      result.priority,
      result.dueDate,
      subTasks.length ? subTasks : undefined,
      resolved.goal?.id,
      resolved.keyResult?.id,
    );
    const assignment = resolved.keyResult
      ? `${resolved.goal?.title} · ${resolved.keyResult.code}`
      : resolved.goal?.title;
    const requestedAssignment = result.objectiveTitle || result.keyResultTitle;
    showToast({
      message: assignment
        ? `已创建任务 · ${assignment}`
        : requestedAssignment
          ? '已创建任务，未找到唯一归属，已放入收件区'
          : '已创建任务 · 未归属',
      type: 'success',
    });
  };

  const prepareConfirmableDraft = (result: Exclude<WorkbenchAiDraft, { intent: 'task' }>): boolean => {
    if (result.intent === 'objective') {
      setDraft({
        intent: 'objective',
        title: result.title,
        description: result.description ?? '',
        keyResults: result.keyResults.map((item, index) => createKeyResult(item.title, index)),
      });
      return true;
    }

    const parentGoal = resolveAiTarget(goals, result.objectiveTitle).goal;
    if (!parentGoal) {
      showToast({ message: '没有唯一匹配到要添加 KR 的目标，请在描述中写明目标名称', type: 'error' });
      return false;
    }
    setDraft({
      intent: 'key_result',
      goalId: parentGoal.id,
      keyResults: result.keyResults.map((item, index) => createKeyResult(item.title, (parentGoal.keyResults?.length ?? 0) + index)),
    });
    return true;
  };

  const handleSubmit = async () => {
    const raw = value.trim();
    if (!raw || loading) return;

    setLoading(true);
    try {
      const result = await parseWorkbenchInput(raw, {
        availableGoals: goals
          .filter(goal => goal.status === 'active')
          .map(goal => ({
            title: goal.title,
            keyResults: (goal.keyResults ?? [])
              .filter(keyResult => keyResult.status !== 'abandoned')
              .map(keyResult => ({ code: keyResult.code, title: keyResult.title })),
          })),
      });
      if (result.intent === 'task') {
        saveTask(result);
        setValue('');
      } else if (prepareConfirmableDraft(result)) {
        setValue('');
      }
    } catch (error) {
      showToast({ message: error instanceof Error ? error.message : 'AI 创建失败', type: 'error' });
    } finally {
      setLoading(false);
    }
  };

  const confirmDraft = () => {
    if (!draft) return;
    const cleanResults = draft.keyResults
      .filter(result => result.title.trim())
      .map((result, index) => ({ ...result, code: `KR${draft.intent === 'key_result' ? (goals.find(goal => goal.id === draft.goalId)?.keyResults?.length ?? 0) + index + 1 : index + 1}`, title: result.title.trim() }));

    if (draft.intent === 'objective') {
      if (!draft.title.trim()) return;
      onCreateGoal?.({
        title: draft.title.trim(),
        description: draft.description.trim() || undefined,
        keyResults: cleanResults,
      });
      showToast({ message: `已创建目标：${draft.title.trim()}`, type: 'success' });
    } else {
      onAppendKeyResults?.(draft.goalId, cleanResults);
      showToast({ message: `已添加 ${cleanResults.length} 个 KR`, type: 'success' });
    }
    setDraft(null);
  };

  return (
    <section className="mb-4 overflow-visible rounded-[14px] border border-[#D9DDE4] bg-white shadow-[0_8px_24px_rgba(17,22,56,0.04)] dark:border-[#2A3039] dark:bg-[#14171C] dark:shadow-none">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3 border-b border-[#E6E8EC] px-4 py-3 dark:border-[#252A32]">
        <div className="flex shrink-0 items-center gap-5">
          <Metric label="今日完成" value={`${todayDoneCount}/${todayTotalCount}`} prominent />
          <span className="h-8 w-px bg-[#E1E4E8] dark:bg-[#2A3039]" />
          <Metric label="本周任务" value={`${weekTotalCount} 项`} />
          <Metric label="今日专注" value={`${pomodoroTodayMinutes} 分`} />
        </div>
        <div className="ml-auto flex items-center">
          {onManageGoals && (
            <button
              type="button"
              onClick={onManageGoals}
              className={`inline-flex h-8 items-center gap-1.5 rounded-[7px] bg-[#F1F2F4] px-2.5 text-[10px] font-medium text-[#59616D] transition-colors hover:bg-[#E7E9ED] hover:text-[#30353D] dark:bg-[#20242B] dark:text-[#A5ADB8] dark:hover:bg-[#2A3039] dark:hover:text-[#E1E5EB] ${KEYBOARD_FOCUS}`}
              aria-haspopup="dialog"
            >
              <Target size={12} />
              目标管理
            </button>
          )}
        </div>
      </div>

      <div className="p-3">
        <div className="mb-2 flex items-center justify-between gap-3 px-0.5">
          <div className="inline-flex items-center gap-1.5 text-[10px] font-medium text-[#5268D9] dark:text-[#AEBBFF]">
            <Sparkles size={12} />
            AI 原生创建
          </div>
          <span className="text-[9px] text-[#8A919C] dark:text-[#7E8794]">任务、目标、KR 共用一个入口</span>
        </div>

        <div className="relative">
          <Sparkles size={14} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-[#5268D9]" />
          <input
            ref={inputRef}
            value={value}
            onChange={event => setValue(event.target.value)}
            onKeyDown={event => {
              if (shouldSubmitOnEnter(event.key, event.nativeEvent.isComposing)) {
                event.preventDefault();
                void handleSubmit();
              } else if (event.key === 'Escape') {
                setValue('');
                inputRef.current?.blur();
              }
            }}
            placeholder="描述任务，或说“创建一个目标并拆成 3 个 KR”"
            disabled={loading}
            className={`h-11 w-full rounded-[9px] border border-[#D9DDE4] bg-[#F7F8FA] pl-9 pr-12 text-[12px] text-[#171A1F] outline-none transition-[border-color,box-shadow,background-color] placeholder:text-[#9AA1AC] disabled:opacity-60 dark:border-[#363D47] dark:bg-[#1A1E25] dark:text-[#E9ECF1] ${INPUT_FOCUS}`}
            aria-label="AI 创建任务、目标或 KR"
          />
          <button
            type="button"
            onClick={() => void handleSubmit()}
            disabled={loading || !value.trim()}
            className={`absolute right-1.5 top-1/2 flex h-8 w-8 -translate-y-1/2 items-center justify-center rounded-[7px] bg-[#3559D6] text-white transition-colors hover:bg-[#2F4FC0] disabled:bg-[#D8DCE5] disabled:text-[#949BA6] dark:disabled:bg-[#303640] dark:disabled:text-[#737C88] ${KEYBOARD_FOCUS}`}
            aria-label="提交 AI 创建"
          >
            {loading ? <Loader2 size={14} className="animate-spin" /> : <ArrowUp size={14} />}
          </button>
        </div>
        <div className="mt-2 flex items-center gap-1.5 px-1 text-[9px] text-[#7A818C] dark:text-[#8D96A3]">
          <span>Enter 提交，中文输入法选词不会误触发；归属无法唯一确定时，任务进入未归属收件区。</span>
        </div>

        {draft && (
          <DraftEditor
            draft={draft}
            parentGoal={draft.intent === 'key_result' ? goals.find(goal => goal.id === draft.goalId) : undefined}
            onChange={setDraft}
            onCancel={() => setDraft(null)}
            onConfirm={confirmDraft}
          />
        )}
      </div>
    </section>
  );
}

function Metric({ label, value, prominent = false }: { label: string; value: string; prominent?: boolean }) {
  return (
    <div>
      <div className="text-[10px] font-medium text-[#7A818C] dark:text-[#8D96A3]">{label}</div>
      <div className={`mt-0.5 font-mono font-semibold tabular-nums text-[#30353D] dark:text-[#D9DEE6] ${prominent ? 'text-[18px]' : 'text-[12px]'}`}>{value}</div>
    </div>
  );
}

function DraftEditor({ draft, parentGoal, onChange, onCancel, onConfirm }: { draft: ConfirmableDraft; parentGoal?: Goal; onChange: (draft: ConfirmableDraft) => void; onCancel: () => void; onConfirm: () => void }) {
  const updateResult = (id: string, title: string) => onChange({
    ...draft,
    keyResults: draft.keyResults.map(result => result.id === id ? { ...result, title } : result),
  });

  return (
    <div className="mt-3 rounded-[10px] border border-[#DDE1E7] bg-[#FAFAFB] p-3 dark:border-[#303640] dark:bg-[#181B21]">
      <div className="mb-3 flex items-start gap-2">
        <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-[8px] bg-[#E9EDFF] text-[#3559D6] dark:bg-[#202639] dark:text-[#AEBBFF]"><Sparkles size={13} /></span>
        <div className="min-w-0 flex-1">
          <div className="text-[11px] font-semibold text-[#30353D] dark:text-[#D9DEE6]">{draft.intent === 'objective' ? '目标草稿' : `KR 草稿 · ${parentGoal?.title ?? ''}`}</div>
          <div className="mt-0.5 text-[9px] text-[#8A919C]">AI 已识别创建意图，修改后确认保存</div>
        </div>
        <button type="button" onClick={onCancel} className="rounded-[5px] p-1 text-[#8A919C] hover:bg-[#ECEEF1] dark:hover:bg-[#242932]" aria-label="取消草稿"><X size={13} /></button>
      </div>

      {draft.intent === 'objective' && (
        <div className="mb-2 grid gap-2 sm:grid-cols-[minmax(0,0.8fr)_minmax(0,1.2fr)]">
          <input value={draft.title} onChange={event => onChange({ ...draft, title: event.target.value })} className={`h-9 rounded-[7px] border border-[#D9DDE4] bg-white px-3 text-[12px] font-semibold text-[#171A1F] outline-none dark:border-[#363D47] dark:bg-[#14171C] dark:text-[#E9ECF1] ${INPUT_FOCUS}`} placeholder="目标标题" />
          <input value={draft.description} onChange={event => onChange({ ...draft, description: event.target.value })} className={`h-9 rounded-[7px] border border-[#D9DDE4] bg-white px-3 text-[11px] text-[#171A1F] outline-none dark:border-[#363D47] dark:bg-[#14171C] dark:text-[#E9ECF1] ${INPUT_FOCUS}`} placeholder="目标说明（可选）" />
        </div>
      )}

      <div className="space-y-1.5">
        {draft.keyResults.map((result, index) => (
          <div key={result.id} className="flex items-center gap-2">
            <span className="w-8 shrink-0 font-mono text-[9px] font-semibold text-[#69717D] dark:text-[#98A1AD]">KR{index + 1}</span>
            <input value={result.title} onChange={event => updateResult(result.id, event.target.value)} className={`h-8 min-w-0 flex-1 rounded-[7px] border border-[#D9DDE4] bg-white px-3 text-[11px] text-[#171A1F] outline-none dark:border-[#363D47] dark:bg-[#14171C] dark:text-[#E9ECF1] ${INPUT_FOCUS}`} />
            <button type="button" onClick={() => onChange({ ...draft, keyResults: draft.keyResults.filter(item => item.id !== result.id) })} className="rounded-[5px] p-1 text-[#8A919C] hover:bg-[#FBEFEF] hover:text-[#A33D3D] dark:hover:bg-[#2C2023]" aria-label={`删除 KR${index + 1}`}><X size={12} /></button>
          </div>
        ))}
      </div>

      <div className="mt-3 flex justify-end gap-2">
        <button type="button" onClick={onCancel} className="h-8 rounded-[7px] px-3 text-[11px] text-[#69717D] hover:bg-[#ECEEF1] dark:text-[#98A1AD] dark:hover:bg-[#242932]">取消</button>
        <button type="button" onClick={onConfirm} disabled={draft.intent === 'objective' && !draft.title.trim()} className="inline-flex h-8 items-center gap-1 rounded-[7px] bg-[#20242B] px-3 text-[11px] font-medium text-white hover:bg-[#343A43] disabled:opacity-40 dark:bg-[#E9ECF1] dark:text-[#171A1F]"><Check size={12} />确认创建</button>
      </div>
    </div>
  );
}
