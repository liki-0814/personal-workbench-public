import { useState, useEffect, useCallback, useRef } from 'react';
import { showToast } from '@/shell';
import * as jobsApi from '../api';
import type { JobInfo } from '../types';

const POLL_INTERVAL_MS = 5000;

export type JobPendingAction = {
  name: string;
  action: 'run' | 'toggle' | 'delete';
} | null;

export function useJobs() {
  const [jobs, setJobs] = useState<JobInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<JobPendingAction>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchJobs = useCallback(async (showLoading = false) => {
    if (showLoading) setLoading(true);
    try {
      const data = await jobsApi.fetchJobs();
      setJobs(data.jobs ?? []);
      setError(null);
      return true;
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '加载调度任务失败');
      return false;
    } finally {
      setLoading(false);
    }
  }, []);

  // Auto-poll when any job is running — update duration / detect completion
  useEffect(() => {
    const hasRunning = jobs.some(j => j.status === 'running');
    if (hasRunning && !pollRef.current) {
      pollRef.current = setInterval(() => { void fetchJobs(); }, POLL_INTERVAL_MS);
    } else if (!hasRunning && pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
    return () => {
      if (pollRef.current) {
        clearInterval(pollRef.current);
        pollRef.current = null;
      }
    };
  }, [jobs, fetchJobs]);

  useEffect(() => {
    void fetchJobs(true);
    return () => {
      if (refreshTimerRef.current) clearTimeout(refreshTimerRef.current);
    };
  }, [fetchJobs]);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const data = await jobsApi.refreshJobs();
      setJobs(data.jobs ?? []);
      setError(null);
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : '刷新调度任务失败';
      setError(message);
      showToast({ message, type: 'error' });
    } finally {
      setLoading(false);
    }
  }, []);

  const runJob = useCallback(async (name: string) => {
    setPendingAction({ name, action: 'run' });
    try {
      await jobsApi.runJob(name);
      showToast({ message: `已开始执行 ${name}`, type: 'success' });
      if (refreshTimerRef.current) clearTimeout(refreshTimerRef.current);
      refreshTimerRef.current = setTimeout(() => { void fetchJobs(); }, 1000);
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : '执行任务失败';
      setError(message);
      showToast({ message, type: 'error' });
    } finally {
      setPendingAction(null);
    }
  }, [fetchJobs]);

  const toggleJob = useCallback(async (name: string) => {
    setPendingAction({ name, action: 'toggle' });
    try {
      await jobsApi.toggleJob(name);
      await fetchJobs();
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : '更新任务状态失败';
      setError(message);
      showToast({ message, type: 'error' });
    } finally {
      setPendingAction(null);
    }
  }, [fetchJobs]);

  const deleteJob = useCallback(async (name: string) => {
    setPendingAction({ name, action: 'delete' });
    try {
      await jobsApi.deleteJob(name);
      await fetchJobs();
      showToast({ message: `已删除 ${name}`, type: 'success' });
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : '删除任务失败';
      setError(message);
      showToast({ message, type: 'error' });
    } finally {
      setPendingAction(null);
    }
  }, [fetchJobs]);

  const getLogs = useCallback((name: string, signal?: AbortSignal) => jobsApi.fetchJobLogs(name, signal), []);

  const getLogContent = useCallback(
    (name: string, file: string, signal?: AbortSignal) => jobsApi.fetchJobLogContent(name, file, signal),
    [],
  );

  const deleteLog = useCallback((name: string, file: string) => jobsApi.deleteJobLog(name, file), []);

  return {
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
  };
}
