import { useState } from 'react';
import { ChevronDown, ChevronRight, Play, Pause, Power, Trash2, FileText } from 'lucide-react';
import type { JobInfo } from '../../types';
import { formatDuration, formatRelTime, formatFutureRelTime } from '@/core/utils/time';

function StatusDot({ status }: { status: JobInfo['status'] }) {
  const cls = {
    idle: 'bg-[#10B981]',
    running: 'bg-[#3B82F6] animate-pulse',
    error: 'bg-[#EF4444]',
    disabled: 'bg-gray-400',
  }[status];
  return <span className={`inline-block w-2 h-2 rounded-full flex-shrink-0 ${cls}`} />;
}

export default function JobCard({ job, onRun, onToggle, onDelete, onViewLogs, pending }: {
  job: JobInfo;
  onRun: () => void;
  onToggle: () => void;
  onDelete: () => void;
  onViewLogs: () => void;
  pending: boolean;
}) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="border border-gray-100 dark:border-[#2A2D35] rounded-[10px] overflow-hidden">
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full flex items-center gap-2.5 px-3 py-2.5 text-left hover:bg-gray-50 dark:hover:bg-[#1A1D24]/50 transition-colors"
      >
        {expanded ? <ChevronDown size={12} className="text-gray-400 flex-shrink-0" /> : <ChevronRight size={12} className="text-gray-400 flex-shrink-0" />}
        <StatusDot status={job.status} />
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-[13px] font-medium text-gray-900 dark:text-[#E5E7EB] truncate">{job.name}</span>
            {job.type === 'agent' && <span className="text-[9px] px-1 py-0.5 rounded bg-purple-100 dark:bg-purple-900/20 text-purple-600 dark:text-purple-400">AI</span>}
            {job.verification === 'verified' && <span className="text-[9px] text-emerald-600 dark:text-emerald-400">已测试</span>}
            {job.status === 'running' && job.running && (
              <span className="text-[10px] text-[#3B82F6] font-medium">
                {formatDuration(Date.now() - new Date(job.running.startedAt).getTime())}
              </span>
            )}
          </div>
          <div className="flex items-center gap-1.5 mt-0.5">
            <span className="text-[11px] text-gray-500 dark:text-[#6B7280]">{job.cronHuman}</span>
            {job.lastRun && (
              <>
                <span className="text-[11px] text-gray-300 dark:text-[#3B4049]">·</span>
                <span className="text-[11px] text-gray-500 dark:text-[#6B7280]">
                  {formatRelTime(job.lastRun.time)} {job.lastRun.exitCode === 0 ? '✓' : '✗'} {formatDuration(job.lastRun.duration)}
                </span>
              </>
            )}
            {job.enabled && job.nextRun && (
              <>
                <span className="text-[11px] text-gray-300 dark:text-[#3B4049]">·</span>
                <span className="text-[11px] text-gray-500 dark:text-[#6B7280]">下次 {formatFutureRelTime(job.nextRun)}</span>
              </>
            )}
          </div>
        </div>
      </button>

      {expanded && (
        <div className="px-3 pb-3 pt-1 border-t border-gray-50 dark:border-[#1E2028]">
          <code className="block text-[11px] text-gray-600 dark:text-[#9CA3AF] bg-gray-50 dark:bg-[#13161C] px-2 py-1.5 rounded-md mb-2 whitespace-pre-wrap max-h-[120px] overflow-y-auto">
            {job.type === 'agent' ? `🤖 ${job.prompt}` : job.command}
          </code>
          {job.type === 'agent' && (job.model || job.cwd) && (
            <div className="text-[10px] text-gray-400 dark:text-[#4B5563] mb-1">
              {job.model && <span>model: {job.model}</span>}
              {job.model && job.cwd && <span> · </span>}
              {job.cwd && <span>cwd: {job.cwd}</span>}
            </div>
          )}
          {job.group && (
            <div className="text-[10px] text-gray-400 dark:text-[#4B5563] mb-2">group: {job.group}</div>
          )}
          <div className="flex items-center gap-1">
            <button onClick={onRun} disabled={job.status === 'running' || pending}
              className="h-6 px-2 rounded-md text-[11px] font-medium flex items-center gap-1 text-gray-600 dark:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] disabled:opacity-40 transition-colors">
              <Play size={10} /> 执行
            </button>
            <button onClick={onToggle} disabled={pending}
              className="h-6 px-2 rounded-md text-[11px] font-medium flex items-center gap-1 text-gray-600 dark:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] disabled:opacity-40 transition-colors">
              {job.enabled ? <><Pause size={10} /> 暂停</> : <><Power size={10} /> 启用</>}
            </button>
            <button onClick={onViewLogs}
              className="h-6 px-2 rounded-md text-[11px] font-medium flex items-center gap-1 text-gray-600 dark:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors">
              <FileText size={10} /> 日志
            </button>
            <button onClick={onDelete} disabled={pending}
              className="h-6 px-2 rounded-md text-[11px] font-medium flex items-center gap-1 text-[#EF4444] hover:bg-red-50 dark:hover:bg-red-900/10 disabled:opacity-40 transition-colors ml-auto">
              <Trash2 size={10} /> 删除
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
