import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { AlertCircle, ArrowUp, ChevronRight, Folder, Loader2, RefreshCw, X } from 'lucide-react';
import { apiFetch } from '@/core/utils';
import { filterHidden, useShowHiddenFiles } from '@/core/utils/showHiddenFiles';
import { normalizeFsBase, useLocalConfig } from '@/core/config';
import { useModalDialog } from '@/shell';
import type { LocalProject } from '../../types';
import './precision.css';

interface DirectoryListResponse {
  success: boolean;
  path: string;
  entries: Array<{ name: string; type: 'file' | 'directory'; size?: number }>;
}

export interface ResolvedDirectory {
  canonicalPath: string;
  displayPath: string;
  fsBase: string;
  readable: boolean;
}

interface Props {
  open: boolean;
  projects: LocalProject[];
  initialPath?: string;
  onClose: () => void;
  onConfirm: (directory: ResolvedDirectory) => void;
}

function parentPath(path: string): string {
  const normalized = path.replace(/\/+$/, '');
  const index = normalized.lastIndexOf('/');
  if (index <= 0) return '/';
  return normalized.slice(0, index);
}

export default function DirectoryPickerModal({ open, projects, initialPath, onClose, onConfirm }: Props) {
  const localConfig = useLocalConfig();
  const showHidden = useShowHiddenFiles();
  const fsBase = normalizeFsBase(localConfig.tools?.fsBase);
  const inputRef = useRef<HTMLInputElement>(null);
  const previousFsBaseRef = useRef(fsBase);
  const pickerWasOpenRef = useRef(false);
  const confirmAbortRef = useRef<AbortController | null>(null);
  const confirmInFlightRef = useRef(false);
  const close = useCallback(() => {
    confirmAbortRef.current?.abort();
    confirmAbortRef.current = null;
    confirmInFlightRef.current = false;
    setConfirming(false);
    onClose();
  }, [onClose]);
  const dialogRef = useModalDialog({ open, onClose: close, initialFocusRef: inputRef });
  const [path, setPath] = useState(initialPath || '.');
  const [resolvedPath, setResolvedPath] = useState('');
  const [entries, setEntries] = useState<DirectoryListResponse['entries']>([]);
  const [loading, setLoading] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState('');

  const loadDirectory = useCallback(async (target: string) => {
    setLoading(true);
    setError('');
    try {
      const result = await apiFetch<DirectoryListResponse>(`/api/fs/list?path=${encodeURIComponent(target || '.')}`);
      const directories = result.entries.filter(entry => entry.type === 'directory');
      setEntries(directories);
      setResolvedPath(result.path);
      setPath(result.path);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '无法读取该文件夹');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    const isOpening = open && !pickerWasOpenRef.current;
    const fsBaseChanged = fsBase !== previousFsBaseRef.current;
    previousFsBaseRef.current = fsBase;
    pickerWasOpenRef.current = open;
    if (!open) return;
    const target = fsBaseChanged && !isOpening ? fsBase : initialPath || fsBase;
    setPath(target);
    void loadDirectory(target);
  }, [fsBase, initialPath, loadDirectory, open, projects]);

  useEffect(() => () => {
    confirmAbortRef.current?.abort();
    confirmAbortRef.current = null;
    confirmInFlightRef.current = false;
  }, []);

  if (!open) return null;

  const visibleEntries = filterHidden(entries, showHidden);

  const confirm = async () => {
    if (confirmInFlightRef.current) return;
    confirmInFlightRef.current = true;
    const controller = new AbortController();
    confirmAbortRef.current = controller;
    setConfirming(true);
    setError('');
    try {
      const directory = await apiFetch<ResolvedDirectory>('/api/fs/resolve-directory', {
        method: 'POST',
        body: JSON.stringify({ path }),
        signal: controller.signal,
      });
      if (controller.signal.aborted) return;
      if (!directory.readable) throw new Error('该文件夹不可读');
      onConfirm(directory);
    } catch (reason) {
      if (controller.signal.aborted) return;
      setError(reason instanceof Error ? reason.message : '无法使用该文件夹');
    } finally {
      if (confirmAbortRef.current === controller) {
        confirmAbortRef.current = null;
        confirmInFlightRef.current = false;
        setConfirming(false);
      }
    }
  };

  return createPortal(
    <div className="workspace-picker-backdrop" onMouseDown={event => {
      if (event.target === event.currentTarget) close();
    }}>
      <section
        ref={dialogRef}
        className="workspace-picker-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="workspace-picker-title"
        tabIndex={-1}
      >
        <header>
          <div>
            <small>新建会话</small>
            <h2 id="workspace-picker-title">选择工作文件夹</h2>
          </div>
          <button type="button" onClick={close} aria-label="关闭文件夹选择"><X size={17} /></button>
        </header>

        {projects.length > 0 && (
          <div className="workspace-picker-recents">
            <span>最近项目</span>
            <div>
              {projects.slice(0, 5).map(project => (
                <button key={project.id} type="button" onClick={() => void loadDirectory(project.canonicalPath)}>
                  <Folder size={14} />
                  <span>{project.name}</span>
                  <small>{project.displayPath}</small>
                </button>
              ))}
            </div>
          </div>
        )}

        <div className="workspace-picker-pathbar">
          <button type="button" onClick={() => void loadDirectory(parentPath(resolvedPath || path))} title="上一级"><ArrowUp size={15} /></button>
          <input
            ref={inputRef}
            value={path}
            onChange={event => setPath(event.target.value)}
            onKeyDown={event => {
              if (event.key === 'Enter') {
                event.preventDefault();
                void loadDirectory(path);
              }
            }}
            aria-label="工作文件夹路径"
          />
          <button type="button" onClick={() => void loadDirectory(path)} title="刷新"><RefreshCw size={15} /></button>
        </div>

        <div className="workspace-picker-list" aria-busy={loading}>
          {loading ? (
            <div className="workspace-picker-state"><Loader2 size={18} className="animate-spin" />正在读取文件夹…</div>
          ) : visibleEntries.length === 0 ? (
            <div className="workspace-picker-state">这个位置没有子文件夹</div>
          ) : visibleEntries.map(entry => (
            <button key={entry.name} type="button" onClick={() => void loadDirectory(`${resolvedPath.replace(/\/$/, '')}/${entry.name}`)}>
              <Folder size={16} />
              <span>{entry.name}</span>
              <ChevronRight size={14} />
            </button>
          ))}
        </div>

        {error && <div className="workspace-picker-error" role="alert"><AlertCircle size={14} />{error}</div>}

        <footer>
          <span title={resolvedPath || path}>{resolvedPath || path}</span>
          <div>
            <button type="button" onClick={close}>取消</button>
            <button type="button" className="is-primary" disabled={loading || confirming || !path.trim()} onClick={() => void confirm()}>
              {confirming && <Loader2 size={14} className="animate-spin" />}
              使用此文件夹
            </button>
          </div>
        </footer>
      </section>
    </div>,
    document.body,
  );
}
