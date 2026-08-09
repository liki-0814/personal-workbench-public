import { Calendar, ExternalLink, Target } from 'lucide-react';
import type { TodoItem, Goal } from '../../types';
import { formatRelativeDate } from '../../utils/grouping';
import { RELATIVE_TONE_COLOR } from './taskTokens';

interface Props {
  todo: TodoItem;
  goals?: Goal[];
}

/**
 * Inline chip row showing due date / scheduled time / sub-task progress /
 * linked objective and key result. Used by task rows in the workbench.
 * (workbench tab widget). Returns null when there's nothing to show.
 */
export default function TaskMetaChips({ todo, goals }: Props) {
  const dueRaw = todo.dueDate ? formatRelativeDate(todo.dueDate) : null;
  const due = (dueRaw && dueRaw.tone === 'overdue' && todo.completed) ? null : dueRaw;
  const subTasks = todo.subTasks || [];
  const subDone = subTasks.filter(s => s.completed).length;
  const subTotal = subTasks.length;
  const linkedGoal = todo.goalId ? goals?.find(g => g.id === todo.goalId) : null;
  const linkedKeyResult = todo.keyResultId ? linkedGoal?.keyResults?.find(result => result.id === todo.keyResultId) : null;

  const deliverableUrl = todo.deliverableUrl?.trim();
  const hasClickableDeliverable = !!deliverableUrl && /^https?:\/\//i.test(deliverableUrl);
  const hasAny = !!(due || subTotal > 0 || linkedGoal || deliverableUrl);
  if (!hasAny) return null;

  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-2">
      {due && (
        <span className={`inline-flex items-center gap-1 rounded-[7px] border px-2 py-1 font-mono text-[11px] tabular-nums ${RELATIVE_TONE_COLOR[due.tone]}`}>
          <Calendar size={11} /> {due.label}
        </span>
      )}
      {subTotal > 0 && (
        <span className="font-mono text-[11px] tabular-nums text-[#69717D] dark:text-[#98A1AD]">{subDone}/{subTotal} 步</span>
      )}
      {linkedGoal && (
        <span className="inline-flex max-w-[16rem] items-center gap-1.5 truncate rounded-[7px] border border-[#D9DDE4] bg-[#F4F5F7] px-2 py-1 text-[11px] font-medium text-[#59616D] dark:border-[#363D47] dark:bg-[#1A1E25] dark:text-[#AAB2BD]" title={linkedKeyResult ? `${linkedGoal.title} · ${linkedKeyResult.title}` : linkedGoal.title}>
          {linkedGoal.emoji ? <span>{linkedGoal.emoji}</span> : <Target size={11} />}
          <span className="truncate">{linkedKeyResult ? `${linkedKeyResult.code} · ${linkedGoal.title}` : linkedGoal.title}</span>
        </span>
      )}
      {deliverableUrl && (
        hasClickableDeliverable ? (
          <a href={deliverableUrl} target="_blank" rel="noreferrer" onClick={event => event.stopPropagation()} className="inline-flex items-center gap-1 rounded-[7px] border border-[#C9D4F6] bg-[#F1F3FC] px-2 py-1 text-[11px] font-medium text-brand hover:border-[#8EA4F8] dark:border-[#343E60] dark:bg-[#202639] dark:text-[#9DB0FF]" title={deliverableUrl}>
            <ExternalLink size={11} />交付产物
          </a>
        ) : (
          <span className="inline-flex items-center gap-1 rounded-[7px] border border-[#D9DDE4] bg-[#F4F5F7] px-2 py-1 text-[11px] text-[#69717D] dark:border-[#363D47] dark:bg-[#1A1E25] dark:text-[#98A1AD]" title={deliverableUrl}>
            <ExternalLink size={11} />交付产物
          </span>
        )
      )}
    </div>
  );
}
