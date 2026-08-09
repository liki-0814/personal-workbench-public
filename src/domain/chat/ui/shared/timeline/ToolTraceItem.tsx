import { useEffect, useRef, useState } from 'react';
import { Wrench, ChevronDown, ChevronRight, Loader2, Check, AlertCircle, Activity, ArrowUpRight } from 'lucide-react';
import type { ToolTrace } from '@/domain/chat/types';
import FailureCard from '../FailureCard';

/**
 * Single tool-call row: condensed one-line summary when collapsed,
 * full args / progress / result when expanded. Failures stay prominent.
 * Shared by the legacy ToolTraceList and the timeline ToolGroupView.
 */
export default function ToolTraceItem({ trace }: { trace: ToolTrace }) {
  const isErr = trace.status === 'error';
  const isRunning = trace.status === 'running';
  const isRecovering = trace.status === 'recovering';
  const isBackgrounded = trace.status === 'backgrounded';
  const hasProgress = (trace.progressLog?.length ?? 0) > 0;
  const [override, setOverride] = useState<boolean | null>(null);
  const open = override !== null ? override : (isRunning && hasProgress);

  const StatusIcon = () => {
    if (isBackgrounded) return <ArrowUpRight size={11} className="precision-status-warning" />;
    if (isRecovering) return <Loader2 size={11} className="precision-status-warning animate-spin" />;
    if (isRunning) return <Loader2 size={11} className="precision-status-primary animate-spin" />;
    if (isErr)     return <AlertCircle size={11} className="precision-status-danger" />;
    return <Check size={11} className="precision-status-success" />;
  };

  return (
    <div className="precision-trace overflow-hidden" data-status={trace.status}>
      <button
        onClick={() => setOverride(!open)}
        className="precision-trace-header w-full flex items-center gap-1.5 px-2 py-1.5 text-[11px]"
      >
        {open ? <ChevronDown size={11} className="precision-muted-icon flex-shrink-0" /> : <ChevronRight size={11} className="precision-muted-icon flex-shrink-0" />}
        <Wrench size={11} className="precision-muted-icon flex-shrink-0" />
        <span className="precision-option-title font-mono truncate">{trace.name}</span>
        <StatusIcon />
        {!open && trace.args && (
          <span className="precision-option-description ml-1 truncate flex-1 text-left">
            {previewArgs(trace.args)}
          </span>
        )}
        {!open && isBackgrounded && (
          <span className="precision-status-warning ml-auto flex-shrink-0 text-[10px]">
            已转后台
          </span>
        )}
        {!open && !isBackgrounded && hasProgress && trace.result === undefined && (
          <span className="precision-status-primary ml-auto flex-shrink-0 text-[10px]">
            {trace.progressLog!.length} 步
          </span>
        )}
        {!open && trace.result !== undefined && (
          <span className={`ml-auto flex-shrink-0 text-[10px] ${isErr ? 'precision-status-danger' : 'precision-option-description'}`}>
            {previewResult(trace.result, isErr)}
          </span>
        )}
      </button>
      {open && (
        <div className="px-2 pb-2 pt-0 space-y-1.5">
          {trace.args && (
            <div>
              <div className="precision-trace-label text-[10px] uppercase tracking-wide mb-0.5">参数</div>
              <pre className="precision-trace-code text-[11px] font-mono whitespace-pre-wrap break-all px-2 py-1.5 max-h-48 overflow-y-auto">{trace.args}</pre>
            </div>
          )}
          {hasProgress && (
            <ProgressSection lines={trace.progressLog!} running={isRunning} />
          )}
          {trace.failure && (
            <FailureCard failure={trace.failure} recoveryPhase={trace.recoveryPhase} compact />
          )}
          {trace.result !== undefined && (
            <div>
              <div className="precision-trace-label text-[10px] uppercase tracking-wide mb-0.5">{isErr ? '错误' : '结果'}</div>
              <pre className={`precision-trace-code text-[11px] font-mono whitespace-pre-wrap break-all px-2 py-1.5 max-h-48 overflow-y-auto ${isErr ? 'is-error' : ''}`}>{trace.result}</pre>
            </div>
          )}
          {isBackgrounded && trace.result === undefined && (
            <div className="precision-status-warning text-[11px] italic">已自动转为后台任务（运行超过 60s），完成后自动通知。</div>
          )}
          {isRunning && trace.result === undefined && !hasProgress && (
            <div className="precision-option-description text-[11px] italic">执行中...</div>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Live progress log for long-running tools (currently code_agent).
 * Visual style borrowed from ThinkingBlock: pulsing icon + step count + monospace scroll.
 * While running, auto-scrolls to bottom so the latest line is always visible.
 */
function ProgressSection({ lines, running }: { lines: string[]; running: boolean }) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (running && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [lines.length, running]);
  return (
    <div>
      <div className="precision-trace-label text-[10px] uppercase tracking-wide mb-0.5 flex items-center gap-1">
        <Activity size={10} className={running ? 'animate-pulse precision-status-primary' : 'precision-muted-icon'} />
        <span>进度</span>
        <span className="precision-status-primary tabular-nums">{lines.length}</span>
      </div>
      <div
        ref={scrollRef}
        className="precision-trace-progress text-[11px] font-mono leading-relaxed px-2 py-1.5 max-h-48 overflow-y-auto space-y-0.5"
      >
        {lines.map((line, i) => (
          <div key={i} className="whitespace-pre-wrap break-all">{line}</div>
        ))}
      </div>
    </div>
  );
}

function previewArgs(args: string): string {
  try {
    const obj = JSON.parse(args);
    const entries = Object.entries(obj);
    if (entries.length === 0) return '';
    const parts = entries.slice(0, 3).map(([k, v]) => {
      const vs = typeof v === 'string' ? v : JSON.stringify(v);
      return `${k}=${vs.length > 20 ? vs.slice(0, 20) + '…' : vs}`;
    });
    return parts.join(' ');
  } catch {
    return args.length > 60 ? args.slice(0, 60) + '…' : args;
  }
}

function previewResult(result: string, isErr: boolean): string {
  if (isErr) {
    const firstLine = result.split('\n')[0].replace(/^Error:\s*/, '');
    return firstLine.length > 40 ? firstLine.slice(0, 40) + '…' : firstLine;
  }
  const len = result.length;
  if (len < 60) return result.replace(/\s+/g, ' ');
  return `${len} 字符`;
}
