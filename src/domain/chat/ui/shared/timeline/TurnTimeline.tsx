import type { TimelineItem } from '@/domain/chat/types';
import ThinkingItemView from './ThinkingItemView';
import ToolGroupView from './ToolGroupView';
import TextItemView from './TextItemView';
import './TurnTimeline.css';

/**
 * Ordered timeline of one assistant turn: thinking segments stream expanded
 * then collapse, tool runs merge into collapsible「操作」groups, and prose
 * stays inline. Expansion is derived from item status — no manual folding.
 */
export default function TurnTimeline({ items, mode = 'all' }: { items: TimelineItem[]; mode?: 'all' | 'process' | 'answer' }) {
  const visible = items.filter(item => {
    if (mode === 'all') return true;
    const isFinal = item.kind === 'text' && (item.phase === 'final' || item.phase === undefined);
    return mode === 'answer' ? isFinal : !isFinal;
  });
  return (
    <div className="agent-turn-timeline" aria-label={mode === 'answer' ? '最终答案' : '执行时间线'}>
      {visible.map(item => {
        let content;
        switch (item.kind) {
          case 'thinking':
            content = <ThinkingItemView item={item} />;
            break;
          case 'tool_group':
            content = <ToolGroupView item={item} />;
            break;
          case 'text':
            content = <TextItemView item={item} />;
            break;
        }
        return (
          <div key={item.id} className={`agent-turn-step agent-turn-step-${item.kind}`}>
            {mode !== 'answer' && <span className="agent-turn-step-node" aria-hidden />}
            <div className="agent-turn-step-content">{content}</div>
          </div>
        );
      })}
    </div>
  );
}
