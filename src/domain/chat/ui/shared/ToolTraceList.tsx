import type { ToolTrace } from '@/domain/chat/types';
import { ToolActivityGroup, type ToolActivityItem } from '@/shell';
import ToolTraceItem from './timeline/ToolTraceItem';

interface Props {
  traces: ToolTrace[];
  compact?: boolean;
}

/**
 * Legacy rendering path for assistant turns produced before the timeline
 * paradigm: normal activity condensed into one live row, failures stay
 * visible. New turns render via TurnTimeline instead.
 */
export default function ToolTraceList({ traces, compact = false }: Props) {
  if (!traces || traces.length === 0) return null;
  const regular = traces.filter(trace => trace.status !== 'error');
  const failed = traces.filter(trace => trace.status === 'error');
  const traceById = new Map(traces.map(trace => [trace.id, trace]));
  const activityItems: ToolActivityItem[] = regular.map(trace => ({
    id: trace.id,
    label: trace.name,
    status: trace.status === 'done'
      ? 'completed'
      : trace.status === 'backgrounded'
        ? 'backgrounded'
        : 'running',
  }));
  return (
    <div className={`flex flex-col gap-1.5 mb-2 ${compact ? 'text-xs' : ''}`}>
      <ToolActivityGroup
        items={activityItems}
        className="precision-trace precision-tool-activity"
        summaryClassName="precision-trace-header w-full flex items-center gap-1.5 px-2 py-1.5 text-[11px]"
        listClassName="precision-tool-activity-history flex flex-col gap-1.5 px-2 pb-2"
        renderItem={item => <ToolTraceItem trace={traceById.get(item.id)!} />}
      />
      {failed.map(trace => <ToolTraceItem key={trace.id} trace={trace} />)}
    </div>
  );
}
