import { useState } from 'react';
import { Brain, ChevronDown } from 'lucide-react';
import type { TimelineThinkingItem } from '@/domain/chat/types';

/**
 * One thinking segment on the timeline. Streams expanded while running,
 * collapses to「深度思考 · Ns」once closed; click to override.
 */
export default function ThinkingItemView({ item }: { item: TimelineThinkingItem }) {
  const [override, setOverride] = useState<boolean | null>(null);
  const running = item.status === 'running';
  const expanded = override !== null ? override : running;
  const durationSec = Math.max(0, Math.round(((item.endedAt ?? Date.now()) - item.startedAt) / 1000));
  return (
    <div className="precision-thinking-block overflow-hidden">
      <button
        onClick={() => setOverride(!expanded)}
        className="precision-thinking-header w-full flex items-center gap-1.5 px-3 py-1.5 text-[11px]"
      >
        <Brain size={12} className={running ? 'animate-pulse' : ''} />
        <span className="font-medium">深度思考</span>
        {!running && (
          <span className="precision-thinking-meta tabular-nums">· {durationSec}s</span>
        )}
        <ChevronDown
          size={12}
          className={`ml-auto transition-transform ${expanded ? 'rotate-180' : ''}`}
        />
      </button>
      {expanded && (
        <div className="precision-thinking-copy px-3 pb-2 text-[12px] leading-relaxed whitespace-pre-wrap font-mono max-h-[300px] overflow-y-auto">
          {item.text || (running ? '模型正在进行内部推理…' : '模型未返回可展示的推理摘要。')}
        </div>
      )}
    </div>
  );
}
