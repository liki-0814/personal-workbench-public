import { useEffect, useRef, useState } from 'react';
import { X } from 'lucide-react';

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
