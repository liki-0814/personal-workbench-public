import { useState } from 'react';
import { ChevronDown, Scale } from 'lucide-react';
import type { DecisionTrace } from '@/domain/chat/types';

export default function DecisionTraceList({ traces }: { traces: DecisionTrace[] }) {
  const [expanded, setExpanded] = useState(false);
  const active = traces.some(trace => trace.status === 'reviewing');
  return (
    <div className="mb-2 rounded-lg border border-indigo-400/20 bg-indigo-500/5 text-xs">
      <button
        type="button"
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-indigo-700 dark:text-indigo-300"
        onClick={() => setExpanded(value => !value)}
      >
        <Scale size={13} />
        <span className="font-medium">决策复核 · {active ? '审议中' : `${traces.length} 次`}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${expanded ? 'rotate-180' : ''}`} />
      </button>
      {expanded && (
        <div className="space-y-2 border-t border-indigo-400/15 px-3 py-2">
          {traces.map(trace => (
            <div key={trace.id}>
              <div className="font-medium">{trace.trigger} · {trace.risk} · {trace.outcome ?? trace.status}</div>
              {trace.rationale && <div className="mt-1 opacity-75">{trace.rationale}</div>}
              {trace.advisors.length > 0 && (
                <div className="mt-1 opacity-60">顾问：{trace.advisors.map(item => item.model).join('、')}</div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
