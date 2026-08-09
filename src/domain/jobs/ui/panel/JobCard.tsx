import { useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { ChevronDown, ChevronRight, Play, Pause, Power, Trash2, FileText, Bot, Terminal } from 'lucide-react';
import type { JobInfo } from '../../types';
import { formatDuration, formatRelTime, formatFutureRelTime } from '@/core/utils/time';

function StatusDot({ status }: { status: JobInfo['status'] }) {
  const cls = {
    idle: 'bg-[#10B981]',
    running: 'bg-[#3B82F6] animate-pulse motion-reduce:animate-none',
    error: 'bg-[#EF4444]',
    disabled: 'bg-gray-400',
  }[status];
  return <span className={`inline-block w-2 h-2 rounded-full flex-shrink-0 ${cls}`} />;
}

function ActionButton({ onClick, disabled, danger, ariaLabel, children }: {
  onClick: () => void;
  disabled?: boolean;
  danger?: boolean;
  ariaLabel?: string;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      aria-label={ariaLabel}
      className={`h-6 px-2 rounded-md text-[11px] font-medium flex items-center gap-1 disabled:opacity-40 transition-colors ${
        danger
          ? 'text-[#EF4444] hover:bg-red-50 dark:hover:bg-red-900/10'
          : 'text-[var(--text-secondary)] hover:bg-gray-100 dark:hover:bg-[#1A1D24]'
      }`}
    >
      {children}
    </button>
  );
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
    <motion.div
      layout
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.98, transition: { duration: 0.15, ease: 'easeOut' } }}
      className="rounded-[14px] border border-[var(--border-subtle)] bg-[var(--surface-3)] dark:bg-[var(--surface-1)] overflow-hidden transition-colors hover:border-[var(--border-hover)]"
    >
      <div className="flex items-center">
        <button
          type="button"
          onClick={() => setExpanded(!expanded)}
          aria-expanded={expanded}
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-2.5 px-3 py-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-brand/25"
        >
        {expanded
          ? <ChevronDown size={12} className="text-gray-400 flex-shrink-0" />
          : <ChevronRight size={12} className="text-gray-400 flex-shrink-0" />}
        <span
          className="w-8 h-8 rounded-[10px] flex items-center justify-center text-white flex-shrink-0 shadow-sm"
          style={{ background: job.type === 'agent' ? 'var(--chart-gradient-2)' : 'var(--chart-gradient-0)' }}
        >
          {job.type === 'agent' ? <Bot size={14} /> : <Terminal size={14} />}
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <StatusDot status={job.status} />
            <span className="text-[13px] font-medium text-[var(--text-primary)] truncate">{job.name}</span>
            {job.type === 'agent' && (
              <span className="text-[9px] px-1 py-0.5 rounded bg-purple-100 dark:bg-purple-900/20 text-purple-600 dark:text-purple-400">AI</span>
            )}
            {job.verification === 'verified' && (
              <span className="text-[9px] text-emerald-600 dark:text-emerald-400">已测试</span>
            )}
            {job.status === 'running' && job.running && (
              <span className="text-[10px] text-[#3B82F6] font-medium tabular-nums">
                {formatDuration(Date.now() - new Date(job.running.startedAt).getTime())}
              </span>
            )}
          </div>
          <div className="flex items-center gap-1.5 mt-0.5">
            <span className="text-[11px] text-[var(--text-secondary)]">{job.cronHuman}</span>
            {job.lastRun && (
              <>
                <span className="text-[11px] text-gray-300 dark:text-[#3B4049]">·</span>
                <span className="text-[11px] text-[var(--text-secondary)]">
                  {formatRelTime(job.lastRun.time)} {job.lastRun.exitCode === 0 ? '✓' : '✗'} {formatDuration(job.lastRun.duration)}
                </span>
              </>
            )}
            {job.enabled && job.nextRun && (
              <>
                <span className="text-[11px] text-gray-300 dark:text-[#3B4049]">·</span>
                <span className="text-[11px] text-[var(--text-secondary)]">下次 {formatFutureRelTime(job.nextRun)}</span>
              </>
            )}
          </div>
        </div>
        </button>
      </div>

      <AnimatePresence initial={false}>
        {expanded && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.15, ease: 'easeOut' }}
          className="px-3 pb-3 pt-1 border-t border-gray-50 dark:border-[#1E2028]"
        >
          <code className="block text-[11px] text-[var(--text-secondary)] bg-gray-50 dark:bg-[#13161C] px-2 py-1.5 rounded-md mb-2 whitespace-pre-wrap max-h-[120px] overflow-y-auto">
            {job.type === 'agent' ? `🤖 ${job.prompt}` : job.command}
          </code>
          {job.type === 'agent' && (job.model || job.cwd) && (
            <div className="text-[10px] text-[var(--text-muted)] mb-1">
              {job.model && <span>model: {job.model}</span>}
              {job.model && job.cwd && <span> · </span>}
              {job.cwd && <span>cwd: {job.cwd}</span>}
            </div>
          )}
          {job.group && (
            <div className="text-[10px] text-[var(--text-muted)] mb-2">group: {job.group}</div>
          )}
          <div className="flex items-center gap-1">
            <ActionButton onClick={onRun} disabled={job.status === 'running' || pending}>
              <Play size={10} /> 执行
            </ActionButton>
            <ActionButton onClick={onToggle} disabled={pending}>
              {job.enabled ? <><Pause size={10} /> 暂停</> : <><Power size={10} /> 启用</>}
            </ActionButton>
            <ActionButton onClick={onViewLogs}>
              <FileText size={10} /> 日志
            </ActionButton>
            <ActionButton danger onClick={onDelete} disabled={pending} ariaLabel={`删除${job.name}`}>
              <Trash2 size={10} /> 删除
            </ActionButton>
          </div>
        </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
}
