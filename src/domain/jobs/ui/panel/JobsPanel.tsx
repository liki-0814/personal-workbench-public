import { useEffect, useRef, useState } from 'react';
import { AlertCircle, Bot, RefreshCw, Play, Pause, Power, Trash2, FileText, ChevronDown, ChevronRight, X } from 'lucide-react';
import { useJobs } from '../../state/store';
import type { JobInfo } from '../../types';

function formatDuration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rs = s % 60;
  if (m < 60) return `${m}m${rs > 0 ? ` ${rs}s` : ''}`;
  const h = Math.floor(m / 60);
  const rm = m % 60;
  return `${h}h${rm > 0 ? ` ${rm}m` : ''}`;
}

function formatRelTime(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const m = Math.floor(diff / 60000);
  if (m < 1) return '刚刚';
  if (m < 60) return `${m}分钟前`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}小时前`;
  const d = Math.floor(h / 24);
  return `${d}天前`;
}

function StatusDot({ status }: { status: JobInfo['status'] }) {
  const cls = {
    idle: 'bg-[#10B981]',
    running: 'bg-[#3B82F6] animate-pulse',
    error: 'bg-[#EF4444]',
    disabled: 'bg-gray-400',
  }[status];
  return <span className={`inline-block w-2 h-2 rounded-full flex-shrink-0 ${cls}`} />;
}

function JobCard({ job, onRun, onToggle, onDelete, onViewLogs, pending }: {
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
          </div>
        </div>
      </button>

      {expanded && (
        <div className="px-3 pb-3 pt-1 border-t border-gray-50 dark:border-[#1E2028]">
          <>
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
            </>
        </div>
      )}
    </div>
  );
}

export function LogViewer({ name, onClose, getLogs, getLogContent, onDeleteLog }: {
  name: string;
  onClose: () => void;
  getLogs: (name: string, signal?: AbortSignal) => Promise<string[]>;
  getLogContent: (name: string, file: string, signal?: AbortSignal) => Promise<string>;
  onDeleteLog: (name: string, file: string) => Promise<void>;
}) {
  const [files, setFiles] = useState<string[]>([]);
  const [content, setContent] = useState('');
  const [activeFile, setActiveFile] = useState('');
  const [loading, setLoading] = useState(true);
  const [contentLoading, setContentLoading] = useState(false);
  const [deletingFile, setDeletingFile] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);
  const requestRef = useRef<AbortController | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    requestRef.current?.abort();
    requestRef.current = controller;
    setLoading(true);
    setError(null);
    void (async () => {
      try {
        const logs = await getLogs(name, controller.signal);
        if (controller.signal.aborted) return;
        setFiles(logs);
        if (logs.length > 0) {
          setActiveFile(logs[0]);
          setContentLoading(true);
          const value = await getLogContent(name, logs[0], controller.signal);
          if (!controller.signal.aborted) setContent(value);
        } else {
          setActiveFile('');
          setContent('');
        }
      } catch (reason) {
        if (!controller.signal.aborted) {
          setError(reason instanceof Error ? reason.message : '加载日志失败');
        }
      } finally {
        if (!controller.signal.aborted) {
          setLoading(false);
          setContentLoading(false);
        }
      }
    })();
    return () => controller.abort();
  }, [getLogContent, getLogs, name, reloadToken]);

  const selectFile = async (file: string) => {
    const controller = new AbortController();
    requestRef.current?.abort();
    requestRef.current = controller;
    setActiveFile(file);
    setContent('');
    setContentLoading(true);
    setError(null);
    try {
      const value = await getLogContent(name, file, controller.signal);
      if (!controller.signal.aborted) setContent(value);
    } catch (reason) {
      if (!controller.signal.aborted) setError(reason instanceof Error ? reason.message : '读取日志失败');
    } finally {
      if (!controller.signal.aborted) setContentLoading(false);
    }
  };

  const handleDeleteLog = async (file: string) => {
    if (!window.confirm(`确定删除日志「${file}」？`)) return;
    setDeletingFile(file);
    setError(null);
    try {
      await onDeleteLog(name, file);
      const next = files.filter(f => f !== file);
      setFiles(next);
      if (activeFile === file) {
        if (next.length > 0) {
          await selectFile(next[0]);
        } else {
          setActiveFile('');
          setContent('');
        }
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '删除日志失败');
    } finally {
      setDeletingFile('');
    }
  };

  return (
    <div className="mt-2 border border-gray-100 dark:border-[#2A2D35] rounded-[10px] overflow-hidden">
      <div className="flex items-center justify-between px-3 py-2 bg-gray-50 dark:bg-[#13161C]">
        <span className="text-[11px] font-medium text-gray-700 dark:text-[#9CA3AF]">{name} - 日志</span>
        <button onClick={onClose} className="text-[11px] text-gray-500 hover:text-gray-700 dark:hover:text-gray-300">关闭</button>
      </div>
      {loading ? (
        <div className="px-3 py-4 text-[11px] text-gray-400">加载中...</div>
      ) : error && files.length === 0 ? (
        <div className="px-3 py-4 text-[11px] text-red-500">
          <p>{error}</p>
          <button type="button" className="mt-2 underline" onClick={() => setReloadToken(value => value + 1)}>重试</button>
        </div>
      ) : files.length === 0 ? (
        <div className="px-3 py-4 text-[11px] text-gray-400">暂无日志</div>
      ) : (
        <>
          <div className="flex gap-1 px-3 py-1.5 overflow-x-auto border-b border-gray-100 dark:border-[#1E2028] items-center">
            {files.slice(0, 10).map(f => (
              <div key={f} className="flex items-center gap-0.5 shrink-0">
                <button
                  onClick={() => selectFile(f)}
                  disabled={contentLoading}
                  className={`px-2 py-0.5 rounded text-[10px] whitespace-nowrap transition-colors ${
                    f === activeFile
                      ? 'bg-[#3B82F6] text-white'
                      : 'text-gray-500 dark:text-[#6B7280] hover:bg-gray-100 dark:hover:bg-[#1A1D24]'
                  }`}
                >
                  {f.replace('.log', '').replace('..', '.')}
                </button>
                <button
                  onClick={() => handleDeleteLog(f)}
                  disabled={deletingFile === f}
                  className="text-gray-300 dark:text-[#3B4049] hover:text-[#EF4444] transition-colors"
                  title="删除此日志"
                >
                  <X size={10} />
                </button>
              </div>
            ))}
          </div>
          {error && <div className="px-3 py-1.5 text-[11px] text-red-500">{error}</div>}
          <pre className="px-3 py-2 text-[11px] leading-relaxed text-[#c9d1d9] bg-[#0d1117] max-h-[300px] overflow-y-auto whitespace-pre-wrap break-all">
            {contentLoading ? '加载中...' : content || '(empty)'}
          </pre>
        </>
      )}
    </div>
  );
}

export default function JobsPanel({ onAskAi }: { onAskAi?: () => void }) {
  const {
    jobs,
    loading,
    error,
    pendingAction,
    refresh,
    runJob,
    toggleJob,
    deleteJob,
    getLogs,
    getLogContent,
    deleteLog,
  } = useJobs();
  const [logTarget, setLogTarget] = useState<string | null>(null);

  return (
    <section className="px-4 py-4 h-full flex flex-col min-h-0">
      <div className="flex items-center justify-between pb-3 mb-3 border-b border-gray-100 dark:border-[#1E2028]">
        <h3 className="text-[13px] font-semibold text-gray-900 dark:text-[#E5E7EB] tracking-[-0.01em]">调度任务</h3>
        <div className="flex items-center gap-1">
          <button
            onClick={() => { void refresh(); }}
            disabled={loading}
            className="w-7 h-7 flex items-center justify-center rounded-[8px] text-gray-400 dark:text-[#4B5563] hover:text-gray-600 dark:hover:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors"
            title="刷新"
          >
            <RefreshCw size={14} className={loading ? 'animate-spin' : ''} />
          </button>
          {onAskAi && <button onClick={onAskAi} className="flex h-7 items-center gap-1.5 rounded-[8px] bg-[#3559D6] px-2.5 text-[11px] font-medium text-white hover:bg-[#2F4FC0]"><Bot size={13} />让 AI 创建</button>}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto min-h-0 space-y-2">
        {error && (
          <div className="flex items-start gap-2 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-[11px] text-red-700 dark:border-red-900/40 dark:bg-red-950/20 dark:text-red-300">
            <AlertCircle size={13} className="mt-0.5 shrink-0" />
            <span className="flex-1">{error}</span>
            <button type="button" className="font-medium underline" onClick={() => { void refresh(); }}>重试</button>
          </div>
        )}
        {loading ? (
          <div className="text-[12px] text-gray-400 dark:text-[#4B5563] py-8 text-center">加载中...</div>
        ) : jobs.length === 0 && !error ? (
          <div className="text-center py-8">
            <p className="text-[12px] text-gray-400 dark:text-[#4B5563]">暂无调度任务</p>
            <p className="text-[11px] text-gray-300 dark:text-[#3B4049] mt-1">由 AI 生成并测试通过后才会出现在这里</p>
            {onAskAi && <button onClick={onAskAi} className="mt-3 text-[11px] font-medium text-[#3559D6] dark:text-[#9DB0FF]">描述你想自动执行的工作</button>}
          </div>
        ) : (
          <>
            {jobs.map(job => (
              <JobCard
                key={job.name}
                job={job}
                onRun={() => { void runJob(job.name); }}
                onToggle={() => { void toggleJob(job.name); }}
                onDelete={() => {
                  if (window.confirm(`确定删除调度任务「${job.name}」？`)) void deleteJob(job.name);
                }}
                onViewLogs={() => setLogTarget(logTarget === job.name ? null : job.name)}
                pending={pendingAction !== null}
              />
            ))}
          </>
        )}

        {logTarget && (
          <LogViewer
            name={logTarget}
            onClose={() => setLogTarget(null)}
            getLogs={getLogs}
            getLogContent={getLogContent}
            onDeleteLog={deleteLog}
          />
        )}
      </div>
    </section>
  );
}
