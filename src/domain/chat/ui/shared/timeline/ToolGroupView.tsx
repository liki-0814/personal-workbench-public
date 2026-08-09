import { useState } from 'react';
import { Wrench, ChevronDown, Loader2, Check } from 'lucide-react';
import type { TimelineToolGroupItem } from '@/domain/chat/types';
import { getToolPhaseSummary } from '@/shell/ui/toolActivity';
import ThinkingItemView from './ThinkingItemView';
import ToolTraceItem from './ToolTraceItem';

/**
 * A run of consecutive tool calls merged into one「操作」group.
 * Expanded while any child runs (「正在操作」), collapses to
 * 「已操作 N次阅读 M次检索」once closed; thinking arriving while the group
 * is open nests inside it.
 */
export default function ToolGroupView({ item }: { item: TimelineToolGroupItem }) {
  const [override, setOverride] = useState<boolean | null>(null);
  const running = item.children.some(child => child.status === 'running' || child.status === 'recovering');
  const expanded = override !== null ? override : running;
  const toolNames = item.children.filter(child => child.kind === 'tool').map(child => child.name);
  const summary = running ? '正在操作' : `已操作 ${getToolPhaseSummary(toolNames)}`;
  return (
    <div className="precision-trace precision-tool-activity overflow-hidden">
      <button
        onClick={() => setOverride(!expanded)}
        className="precision-trace-header w-full flex items-center gap-1.5 px-2 py-1.5 text-[11px]"
      >
        <ChevronDown
          size={11}
          className={`precision-muted-icon flex-shrink-0 transition-transform ${expanded ? '' : '-rotate-90'}`}
        />
        <Wrench size={11} className="precision-muted-icon flex-shrink-0" />
        <span className="precision-option-title">{summary}</span>
        {running
          ? <Loader2 size={11} className="precision-status-primary animate-spin" />
          : <Check size={11} className="precision-status-success" />}
      </button>
      {expanded && (
        <div className="precision-tool-activity-history flex flex-col gap-1.5 px-2 pb-2">
          {item.children.map(child => child.kind === 'tool'
            ? <ToolTraceItem key={child.id} trace={child} />
            : <ThinkingItemView key={child.id} item={child} />)}
        </div>
      )}
    </div>
  );
}
