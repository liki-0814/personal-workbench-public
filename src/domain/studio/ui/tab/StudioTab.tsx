import { useCallback, useEffect, useRef, useState } from 'react';
import { Code2, FileCode2, FileDiff, Loader2, X } from 'lucide-react';
import { readRuntimeTaskReviewDiff, readRuntimeTaskReviewFile } from '../../api';
import { detectLanguage, useStudio } from '../../state/store';
import FileTree from '../components/FileTree';
import FileTabs from '../components/FileTabs';
import EditorPane from '../components/EditorPane';
import DiffPane from '../components/DiffPane';
import type { ReviewDiffResponse, WorkspaceRef } from '../../types';
import '../studio.css';

interface StudioTabProps {
  isDark: boolean;
  workspaceRef?: WorkspaceRef;
  onLoadDiff?: (childId: string, file?: string, reviewRevision?: number) => Promise<ReviewDiffResponse>;
  onClose?: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}

type ReviewFileState =
  | { status: 'loading'; path: string }
  | { status: 'error'; path: string; message: string }
  | { status: 'ready'; path: string; content: string; reviewRevision: number };

function parentDirectory(path: string): string {
  const normalized = path.replace(/\\/g, '/');
  const index = normalized.lastIndexOf('/');
  return index > 0 ? normalized.slice(0, index) : '';
}

export default function StudioTab({ isDark, workspaceRef, onLoadDiff, onClose, onDirtyChange }: StudioTabProps) {
  const studio = useStudio();
  const [view, setView] = useState<'diff' | 'file'>(workspaceRef?.kind === 'diff' ? 'diff' : 'file');
  const [diff, setDiff] = useState<ReviewDiffResponse | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState<string>();
  const [diffReloadKey, setDiffReloadKey] = useState(0);
  const [reviewFile, setReviewFile] = useState<ReviewFileState>();
  const reviewFileAbort = useRef<AbortController | null>(null);
  const reviewDiffAbort = useRef<AbortController | null>(null);

  useEffect(() => {
    if (!workspaceRef) return;
    const filePath = workspaceRef.path && workspaceRef.workspaceRoot && !workspaceRef.path.startsWith('/')
      ? `${workspaceRef.workspaceRoot.replace(/\/$/, '')}/${workspaceRef.path}`
      : workspaceRef.path;
    const root = workspaceRef.workspaceRoot || (filePath ? parentDirectory(filePath) : '');
    if (root && root !== studio.rootDir) studio.setRootDir(root);
    reviewFileAbort.current?.abort();
    setReviewFile(undefined);
    setView(workspaceRef.kind === 'diff' ? 'diff' : 'file');
    if (workspaceRef.kind === 'file' && filePath) void studio.openFile(filePath);
    // The request path is the identity; studio callbacks change as tabs change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceRef?.kind, workspaceRef?.path, workspaceRef?.reviewRevision, workspaceRef?.taskId, workspaceRef?.workspaceRoot]);

  useEffect(() => () => {
    reviewFileAbort.current?.abort();
    reviewDiffAbort.current?.abort();
  }, []);

  useEffect(() => {
    if (workspaceRef?.kind !== 'diff') {
      setDiff(null);
      setDiffError(undefined);
      return;
    }
    if (!workspaceRef.taskId && (!workspaceRef.childId || !onLoadDiff)) {
      setDiff(null);
      setDiffError(undefined);
      return;
    }
    let alive = true;
    reviewDiffAbort.current?.abort();
    const controller = new AbortController();
    reviewDiffAbort.current = controller;
    setDiffLoading(true);
    setDiffError(undefined);
    const request = workspaceRef.taskId
      ? readRuntimeTaskReviewDiff(
          workspaceRef.taskId,
          workspaceRef.path,
          workspaceRef.reviewRevision,
          controller.signal,
        )
      : onLoadDiff!(workspaceRef.childId!, workspaceRef.path, workspaceRef.reviewRevision);
    request.then(response => {
      if (!alive) return;
      if (workspaceRef.reviewRevision != null && response.reviewRevision !== workspaceRef.reviewRevision) {
        throw new Error('审阅 revision 已变化，请打开最新 Review Packet。');
      }
      setDiff(response);
    }).catch(reason => {
      if (alive && !controller.signal.aborted) setDiffError(reason instanceof Error ? reason.message : String(reason));
    }).finally(() => { if (alive) setDiffLoading(false); });
    return () => {
      alive = false;
      controller.abort();
    };
  }, [diffReloadKey, onLoadDiff, workspaceRef?.childId, workspaceRef?.kind, workspaceRef?.path, workspaceRef?.reviewRevision, workspaceRef?.taskId]);

  useEffect(() => {
    onDirtyChange?.(studio.tabs.some(tab => tab.dirty));
  }, [onDirtyChange, studio.tabs]);

  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 's') {
      e.preventDefault();
      if (!reviewFile && studio.activeTabId) studio.saveFile(studio.activeTabId);
    }
  }, [reviewFile, studio]);

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  const requestClose = () => {
    if (studio.tabs.some(tab => tab.dirty) && !window.confirm('Studio 中有未保存的修改，仍要关闭吗？')) return;
    onClose?.();
  };

  const closeTab = (id: string) => {
    const tab = studio.tabs.find(candidate => candidate.id === id);
    if (tab?.dirty && !window.confirm(`${tab.name} 尚未保存，仍要关闭吗？`)) return;
    studio.closeTab(id);
  };

  const openFile = useCallback((path: string) => {
    reviewFileAbort.current?.abort();
    setReviewFile(undefined);
    setView('file');
    void studio.openFile(path);
  }, [studio]);

  const showReferencedFile = () => {
    setView('file');
    if (!workspaceRef?.path) return;
    if (workspaceRef.kind === 'diff' && workspaceRef.taskId) {
      reviewFileAbort.current?.abort();
      const controller = new AbortController();
      reviewFileAbort.current = controller;
      setReviewFile({ status: 'loading', path: workspaceRef.path });
      void readRuntimeTaskReviewFile(workspaceRef.taskId, workspaceRef.path, controller.signal)
        .then(file => {
          if (workspaceRef.reviewRevision != null && file.reviewRevision !== workspaceRef.reviewRevision) {
            throw new Error('审阅 revision 已变化，请重新打开最新改动。');
          }
          setReviewFile({
            status: 'ready',
            path: file.path,
            content: file.content,
            reviewRevision: file.reviewRevision,
          });
        })
        .catch(reason => {
          if (controller.signal.aborted) return;
          setReviewFile({
            status: 'error',
            path: workspaceRef.path,
            message: reason instanceof Error ? reason.message : String(reason),
          });
        });
      return;
    }
    setReviewFile(undefined);
    const filePath = workspaceRef.workspaceRoot && !workspaceRef.path.startsWith('/')
      ? `${workspaceRef.workspaceRoot.replace(/\/$/, '')}/${workspaceRef.path}`
      : workspaceRef.path;
    void studio.openFile(filePath);
  };

  const diffFile = diff?.files.find(file => file.path === workspaceRef?.path) ?? diff?.files[0];
  const canShowDiff = workspaceRef?.kind === 'diff' && Boolean(
    workspaceRef.inlinePatch || workspaceRef.taskId || (workspaceRef.childId && onLoadDiff),
  );

  return (
    <div className="studio-shell flex h-full">
      {/* Sidebar */}
      <div className="studio-sidebar w-[236px] shrink-0 flex flex-col">
        <div className="studio-sidebar-header flex items-center gap-1 p-2">
          <span className="studio-root-label min-w-0 flex-1 truncate px-2 text-xs" title={studio.rootDir}>{studio.rootDir || '执行工作区'}</span>
          {onClose && (
            <button type="button" className="studio-button shrink-0 p-1.5" onClick={requestClose} title="收起 Studio" aria-label="收起 Studio">
              <X size={14} />
            </button>
          )}
        </div>
        <FileTree rootDir={studio.rootDir} onOpenFile={openFile} />
      </div>

      {/* Editor area */}
      <div className="studio-editor flex-1 flex flex-col min-w-0">
        <div className="studio-view-switcher">
          {canShowDiff && <button type="button" className={view === 'diff' ? 'is-active' : ''} onClick={() => setView('diff')}><FileDiff size={12} />改动</button>}
          <button type="button" className={view === 'file' ? 'is-active' : ''} onClick={showReferencedFile}><FileCode2 size={12} />文件</button>
          {workspaceRef?.reviewRevision != null && <small>revision {workspaceRef.reviewRevision}</small>}
          {view === 'diff' && diffError && (
            <button type="button" onClick={() => setDiffReloadKey(key => key + 1)}>重新加载 diff</button>
          )}
        </div>
        {view === 'file' && (reviewFile ? (
          <div className="studio-file-tabs flex shrink-0 items-center px-3 py-1.5 text-xs">
            <span className="truncate">{reviewFile.path.split('/').pop() || reviewFile.path}</span>
            <small className="ml-2 opacity-60">协作者最新 revision · 只读</small>
          </div>
        ) : (
          <FileTabs
            tabs={studio.tabs}
            activeTabId={studio.activeTabId}
            onSelect={id => { setView('file'); studio.setActiveTabId(id); }}
            onClose={closeTab}
          />
        ))}
        {view === 'diff' ? (
          <DiffPane
            patch={diffFile?.patch ?? workspaceRef?.inlinePatch}
            loading={diffLoading}
            error={diffError}
            binary={diffFile?.binary}
            truncated={diffFile?.truncated || diff?.previewTruncated}
          />
        ) : reviewFile?.status === 'loading' ? (
          <div className="studio-diff-state"><Loader2 size={16} className="animate-spin" />读取协作者最新文件…</div>
        ) : reviewFile?.status === 'error' ? (
          <div className="studio-diff-state is-error">
            <span>{reviewFile.message}</span>
            <button type="button" onClick={showReferencedFile}>重新加载文件</button>
          </div>
        ) : reviewFile?.status === 'ready' ? (
          <div className="flex-1 min-h-0">
            <EditorPane
              content={reviewFile.content}
              language={detectLanguage(reviewFile.path.split('/').pop() || reviewFile.path)}
              isDark={isDark}
              onChange={() => undefined}
              onSave={() => undefined}
              readOnly
            />
          </div>
        ) : studio.activeTab ? (
          <div className="flex-1 min-h-0">
            <EditorPane
              content={studio.activeTab.content}
              language={studio.activeTab.language}
              isDark={isDark}
              onChange={(v) => studio.setContent(studio.activeTab!.id, v)}
              onSave={() => studio.saveFile(studio.activeTab!.id)}
            />
          </div>
        ) : (
          <div className="studio-empty flex-1 flex items-center justify-center">
            <div className="text-center">
              <Code2 size={48} className="mx-auto mb-3" />
              <p className="text-sm">
                {studio.rootDir ? '选择一个文件开始编辑' : '请从审阅包或文件引用打开代码'}
              </p>
            </div>
          </div>
        )}
      </div>

    </div>
  );
}
