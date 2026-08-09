import MarkdownRenderer from '@/shell/ui/MarkdownRenderer';
import type { TimelineTextItem } from '@/domain/chat/types';

/**
 * One visible prose paragraph of the turn (intermediate narration or the
 * final answer). Wrapped in the assistant bubble container to keep markdown
 * styling identical to the legacy single-content rendering.
 */
export default function TextItemView({ item }: { item: TimelineTextItem }) {
  if (item.phase === 'discarded') return null;
  if (item.phase === 'draft') {
    return (
      <div className="flex items-center gap-2 py-1 text-xs text-indigo-600/75 dark:text-indigo-300/75">
        <span className="h-1.5 w-1.5 rounded-full bg-current" aria-hidden />
        <span>{item.endedAt ? '候选答案已生成，等待提交' : '正在生成候选答案…'}</span>
      </div>
    );
  }
  if (item.phase === 'narration') {
    return (
      <details className="rounded-md border border-black/5 bg-black/[0.02] px-2.5 py-1.5 text-xs text-black/55 dark:border-white/10 dark:bg-white/[0.03] dark:text-white/55">
        <summary className="cursor-pointer select-none">过程说明</summary>
        <div className="mt-1.5 whitespace-pre-wrap">{item.text}</div>
      </details>
    );
  }
  return (
    <div className="pwb-msg-assistant">
      <MarkdownRenderer content={item.text} />
    </div>
  );
}
