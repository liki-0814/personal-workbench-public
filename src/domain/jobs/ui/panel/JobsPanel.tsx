import { useState } from 'react';
import { AlertCircle, Bot, RefreshCw } from 'lucide-react';
import { IconButton, PanelHeader } from '@/shell';
import { useJobs } from '../../state/store';
import JobCard from './JobCard';
import { LogViewer } from './LogViewer';

export { LogViewer } from './LogViewer';

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
      <PanelHeader
        title="调度任务"
        actions={
          <>
            <IconButton title="刷新" onClick={() => { void refresh(); }} disabled={loading}>
              <RefreshCw size={14} className={loading ? 'animate-spin' : ''} />
            </IconButton>
            {onAskAi && (
              <button
                onClick={onAskAi}
                className="flex h-7 items-center gap-1.5 rounded-[8px] bg-[#3559D6] px-2.5 text-[11px] font-medium text-white hover:bg-[#2F4FC0]"
              >
                <Bot size={13} />让 AI 创建
              </button>
            )}
          </>
        }
      />

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
