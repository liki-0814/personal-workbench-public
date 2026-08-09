import { useState } from 'react';
import { AnimatePresence } from 'framer-motion';
import { AlertCircle, CalendarClock, Plus, RefreshCw } from 'lucide-react';
import { IconButton, ConfirmDialog } from '@/shell';
import { useJobs } from '../../state/store';
import JobCard from './JobCard';
import JobCreateModal from './JobCreateModal';
import { LogViewer } from './LogViewer';

export { LogViewer } from './LogViewer';

export default function JobsPanel() {
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
  const [showCreate, setShowCreate] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);

  return (
    <section className="px-4 py-4 h-full flex flex-col min-h-0">
      <div className="mx-auto w-full max-w-[960px] flex-1 flex flex-col min-h-0">
        {(jobs.length > 0 || error) && (
          <div className="mb-3 flex min-h-8 items-center justify-end gap-1">
            <IconButton title="刷新" onClick={() => { void refresh(); }} disabled={loading}>
              <RefreshCw size={14} className={loading ? 'animate-spin' : ''} />
            </IconButton>
            <button
              type="button"
              onClick={() => setShowCreate(true)}
              className="flex h-8 cursor-pointer items-center gap-1.5 rounded-[9px] bg-brand px-3 text-[12px] font-medium text-white transition-colors hover:bg-brand-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/30 focus-visible:ring-offset-2 dark:focus-visible:ring-offset-[#0F1117]"
            >
              <Plus size={14} aria-hidden="true" />新建调度
            </button>
          </div>
        )}

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
            <div className="flex h-full min-h-[360px] items-center justify-center pb-[6vh]">
              <div className="grid w-full max-w-[720px] grid-cols-[minmax(0,1fr)_300px] gap-8 rounded-[16px] border border-slate-200/80 bg-slate-50/80 p-8 dark:border-[#2A2D35] dark:bg-[#151820]">
                <div className="min-w-0">
                  <div className="flex items-start gap-4">
                  <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-[12px] bg-blue-100 text-brand dark:bg-blue-500/10 dark:text-blue-400">
                    <CalendarClock size={21} aria-hidden="true" />
                  </div>
                  <div className="min-w-0 pt-0.5">
                    <h3 className="text-[15px] font-semibold tracking-[-0.01em] text-slate-900 dark:text-[#E5E7EB]">
                      把重复工作交给自动执行
                    </h3>
                    <p className="mt-1.5 max-w-[430px] text-[12px] leading-5 text-slate-600 dark:text-[#9CA3AF]">
                      描述执行内容和时间，系统会先测试配置，再由你确认启用。
                    </p>
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() => setShowCreate(true)}
                  className="mt-6 flex h-8 shrink-0 cursor-pointer items-center justify-center gap-1.5 rounded-[9px] bg-brand px-3 text-[12px] font-medium text-white transition-colors hover:bg-brand-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/30 focus-visible:ring-offset-2 dark:focus-visible:ring-offset-[#151820]"
                >
                  <Plus size={14} aria-hidden="true" />新建调度
                </button>
              </div>

              <div className="flex items-center gap-3 self-center rounded-[12px] border border-slate-200/90 bg-white px-3.5 py-3 dark:border-[#2A2D35] dark:bg-[#101319]" aria-label="调度创建后的示例">
                <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[9px] bg-slate-100 text-slate-500 dark:bg-white/5 dark:text-[#9CA3AF]">
                  <CalendarClock size={16} aria-hidden="true" />
                </div>
                <div className="min-w-0 flex-1">
                  <p className="truncate text-[12px] font-medium text-slate-700 dark:text-[#D1D5DB]">汇总昨日未完成事项</p>
                  <p className="mt-0.5 text-[11px] text-slate-500 dark:text-[#7D8590]">每天 09:00</p>
                </div>
                <span className="shrink-0 rounded-full bg-slate-100 px-2 py-1 text-[10px] font-medium text-slate-500 dark:bg-white/5 dark:text-[#8D96A3]">未启用</span>
              </div>
              </div>
            </div>
          ) : (
            <div className="grid grid-cols-1 items-start gap-2 lg:grid-cols-2">
              <AnimatePresence initial={false}>
              {jobs.map(job => (
                <JobCard
                  key={job.name}
                  job={job}
                  onRun={() => { void runJob(job.name); }}
                  onToggle={() => { void toggleJob(job.name); }}
                  onDelete={() => setDeleteTarget(job.name)}
                  onViewLogs={() => setLogTarget(logTarget === job.name ? null : job.name)}
                  pending={pendingAction !== null}
                />
              ))}
              </AnimatePresence>
            </div>
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
      </div>

      {showCreate && (
        <JobCreateModal
          onClose={() => setShowCreate(false)}
          onCreated={() => { void refresh(); }}
        />
      )}

      <ConfirmDialog
        open={deleteTarget !== null}
        title="删除调度任务"
        description={deleteTarget ? `确定删除调度任务「${deleteTarget}」？` : ''}
        onConfirm={() => {
          if (deleteTarget) void deleteJob(deleteTarget);
          setDeleteTarget(null);
        }}
        onClose={() => setDeleteTarget(null)}
      />
    </section>
  );
}
