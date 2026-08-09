import { useEffect, useState } from 'react';
import { CheckCircle2, ChevronDown, CircleDashed, Scale, XCircle } from 'lucide-react';
import type { DecisionTrace } from '@/domain/chat/types';

export default function DecisionTraceList({ traces }: { traces: DecisionTrace[] }) {
  const active = traces.some(trace => trace.status === 'reviewing');
  const [expanded, setExpanded] = useState(active);
  useEffect(() => {
    if (active) setExpanded(true);
  }, [active]);
  const currentTrace = traces.find(trace => trace.status === 'reviewing') ?? traces[traces.length - 1];
  const advisors = currentTrace?.advisors ?? [];
  const finished = advisors.filter(item => item.status === 'done' || item.status === 'error').length;
  const awaitingVerdict = active && advisors.length > 0 && finished === advisors.length;
  return (
    <div className="mb-2 rounded-lg border border-indigo-400/20 bg-indigo-500/5 text-xs">
      <button
        type="button"
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-indigo-700 dark:text-indigo-300"
        onClick={() => setExpanded(value => !value)}
      >
        <Scale size={13} />
        <span className="font-medium">决策复核 · {active
          ? awaitingVerdict ? '裁决汇总中' : `审议中${advisors.length ? ` · ${finished}/${advisors.length}` : ''}`
          : `${traces.length} 次`}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${expanded ? 'rotate-180' : ''}`} />
      </button>
      {expanded && (
        <div className="space-y-2 border-t border-indigo-400/15 px-3 py-2">
          {traces.map(trace => (
            <div key={trace.id}>
              <div className="font-medium">{trace.trigger} · {trace.risk} · {trace.outcome ?? trace.status}</div>
              {trace.rationale && <div className="mt-1 opacity-75">{trace.rationale}</div>}
              <div className="mt-2 space-y-1.5">
                {trace.advisors.length === 0 && trace.status === 'reviewing' && (
                  <div className="flex items-center gap-1.5 opacity-60"><CircleDashed size={12} /><span>正在邀请顾问审阅候选答案…</span></div>
                )}
                {trace.advisors.map(item => (
                  <div key={`${item.round ?? 1}:${item.model}`} className="rounded-md border border-indigo-400/10 bg-white/40 px-2 py-1.5 dark:bg-black/10">
                    <div className="flex items-center gap-1.5">
                      {item.status === 'error' ? <XCircle size={12} className="text-red-500" /> : item.status === 'done' ? <CheckCircle2 size={12} className="text-emerald-500" /> : <CircleDashed size={12} />}
                      <span className="font-medium">{item.model}</span>
                      {item.round && <span className="ml-auto opacity-50">第 {item.round} 轮</span>}
                    </div>
                    {item.summary && <div className="mt-1 whitespace-pre-wrap text-black/60 dark:text-white/60">{item.summary}</div>}
                  </div>
                ))}
                {trace.status === 'reviewing' && trace.advisors.length > 0 && trace.advisors.every(item => item.status === 'done' || item.status === 'error') && (
                  <div className="flex items-center gap-1.5 opacity-60"><CircleDashed size={12} /><span>顾问审阅完成，正在汇总裁决…</span></div>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
