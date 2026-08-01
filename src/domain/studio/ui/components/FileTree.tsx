import { useState, useCallback, useEffect, useRef } from 'react';
import { ChevronRight, ChevronDown, File, Folder, FolderOpen, Plus, FilePlus, Trash2 } from 'lucide-react';
import type { FsEntry } from '../../types';
import { useShowHiddenFiles, filterHidden } from '@/core/utils/showHiddenFiles';
import { showToast } from '@/shell';
import { createDirectory, deletePath, listDirectory, writeTextFile } from '../../api';

interface TreeNodeProps {
  entry: FsEntry;
  path: string;
  depth: number;
  showHidden: boolean;
  onOpenFile: (path: string) => void;
  onRefresh?: () => void;
}

function TreeNode({ entry, path, depth, showHidden, onOpenFile, onRefresh }: TreeNodeProps) {
  const [expanded, setExpanded] = useState(false);
  const [children, setChildren] = useState<FsEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mutating, setMutating] = useState(false);
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null);
  const requestRef = useRef<AbortController | null>(null);

  const fullPath = `${path}/${entry.name}`;
  const isDir = entry.type === 'directory';

  const loadChildren = useCallback(async () => {
    if (!isDir) return;
    const controller = new AbortController();
    requestRef.current?.abort();
    requestRef.current = controller;
    setLoading(true);
    setError(null);
    try {
      const entries = await listDirectory(fullPath, controller.signal);
      if (controller.signal.aborted) return;
      const sorted = filterHidden(entries, showHidden).sort((a, b) => {
        if (a.type !== b.type) return a.type === 'directory' ? -1 : 1;
        return a.name.localeCompare(b.name);
      });
      setChildren(sorted);
    } catch (reason) {
      if (!controller.signal.aborted) setError(reason instanceof Error ? reason.message : '无法读取目录');
    } finally {
      if (!controller.signal.aborted) setLoading(false);
    }
  }, [fullPath, isDir, showHidden]);

  const toggle = useCallback(() => {
    if (!isDir) {
      onOpenFile(fullPath);
      return;
    }
    if (!expanded) loadChildren();
    setExpanded(e => !e);
  }, [isDir, expanded, loadChildren, onOpenFile, fullPath]);

  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setContextMenu({ x: e.clientX, y: e.clientY });
  }, []);

  useEffect(() => {
    if (!contextMenu) return;
    const close = () => setContextMenu(null);
    window.addEventListener('click', close);
    return () => window.removeEventListener('click', close);
  }, [contextMenu]);

  useEffect(() => {
    if (expanded && isDir) loadChildren();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showHidden]);

  useEffect(() => () => requestRef.current?.abort(), []);

  const handleNewFile = async () => {
    const name = prompt('文件名:');
    if (!name) return;
    const target = isDir ? fullPath : path;
    setMutating(true);
    try {
      await writeTextFile(`${target}/${name}`, '');
      if (isDir && expanded) await loadChildren();
      onRefresh?.();
      showToast({ message: `已新建 ${name}`, type: 'success' });
    } catch (reason) {
      showToast({ message: reason instanceof Error ? reason.message : '新建文件失败', type: 'error' });
    } finally {
      setMutating(false);
    }
  };

  const handleNewFolder = async () => {
    const name = prompt('文件夹名:');
    if (!name) return;
    const target = isDir ? fullPath : path;
    setMutating(true);
    try {
      await createDirectory(`${target}/${name}`);
      if (isDir && expanded) await loadChildren();
      onRefresh?.();
      showToast({ message: `已新建 ${name}`, type: 'success' });
    } catch (reason) {
      showToast({ message: reason instanceof Error ? reason.message : '新建文件夹失败', type: 'error' });
    } finally {
      setMutating(false);
    }
  };

  const handleDelete = async () => {
    if (!confirm(`删除 ${entry.name}？`)) return;
    setMutating(true);
    try {
      await deletePath(fullPath);
      onRefresh?.();
      showToast({ message: `已删除 ${entry.name}`, type: 'success' });
    } catch (reason) {
      showToast({ message: reason instanceof Error ? reason.message : '删除失败', type: 'error' });
    } finally {
      setMutating(false);
    }
  };

  return (
    <div>
      <div
        className="studio-tree-row flex items-center gap-1 px-2 cursor-pointer text-xs transition-colors"
        style={{ paddingLeft: `${depth * 16 + 8}px` }}
        onClick={toggle}
        onContextMenu={handleContextMenu}
      >
        {isDir ? (
          expanded ? <ChevronDown size={14} className="shrink-0 opacity-50" /> : <ChevronRight size={14} className="shrink-0 opacity-50" />
        ) : (
          <span className="w-3.5 shrink-0" />
        )}
        {isDir ? (
          expanded ? <FolderOpen size={14} className="shrink-0 text-amber-400" /> : <Folder size={14} className="shrink-0 text-amber-400" />
        ) : (
          <File size={14} className="shrink-0 opacity-60" />
        )}
        <span className="truncate">{entry.name}</span>
      </div>

      {contextMenu && (
        <div
          className="studio-context-menu fixed z-50 py-1 min-w-[160px]"
          style={{ left: contextMenu.x, top: contextMenu.y }}
        >
          <button disabled={mutating} className="studio-context-action w-full px-3 py-1.5 text-sm text-left flex items-center gap-2 disabled:opacity-40" onClick={() => { void handleNewFile(); }}>
            <FilePlus size={14} /> 新建文件
          </button>
          <button disabled={mutating} className="studio-context-action w-full px-3 py-1.5 text-sm text-left flex items-center gap-2 disabled:opacity-40" onClick={() => { void handleNewFolder(); }}>
            <Plus size={14} /> 新建文件夹
          </button>
          <div className="studio-context-divider my-1" />
          <button disabled={mutating} className="studio-context-action studio-context-danger w-full px-3 py-1.5 text-sm text-left flex items-center gap-2 disabled:opacity-40" onClick={() => { void handleDelete(); }}>
            <Trash2 size={14} /> 删除
          </button>
        </div>
      )}

      {expanded && !loading && children.map(child => (
        <TreeNode
          key={child.name}
          entry={child}
          path={fullPath}
          depth={depth + 1}
          showHidden={showHidden}
          onOpenFile={onOpenFile}
          onRefresh={() => loadChildren()}
        />
      ))}
      {expanded && loading && (
        <div className="studio-tree-status text-xs py-1" style={{ paddingLeft: `${(depth + 1) * 16 + 8}px` }}>
          加载中...
        </div>
      )}
      {expanded && !loading && error && (
        <div className="studio-tree-status flex items-center gap-2 py-1 text-xs text-red-500" style={{ paddingLeft: `${(depth + 1) * 16 + 8}px` }}>
          <span>{error}</span>
          <button type="button" className="underline" onClick={() => { void loadChildren(); }}>重试</button>
        </div>
      )}
      {expanded && !loading && !error && children.length === 0 && (
        <div className="studio-tree-status py-1 text-xs" style={{ paddingLeft: `${(depth + 1) * 16 + 8}px` }}>文件夹为空</div>
      )}
    </div>
  );
}

interface FileTreeProps {
  rootDir: string;
  onOpenFile: (path: string) => void;
}

export default function FileTree({ rootDir, onOpenFile }: FileTreeProps) {
  const [entries, setEntries] = useState<FsEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const showHidden = useShowHiddenFiles();
  const requestRef = useRef<AbortController | null>(null);

  const load = useCallback(async () => {
    if (!rootDir) return;
    const controller = new AbortController();
    requestRef.current?.abort();
    requestRef.current = controller;
    setLoading(true);
    setError(null);
    try {
      const next = await listDirectory(rootDir, controller.signal);
      if (controller.signal.aborted) return;
      setEntries(next.sort((a, b) => {
        if (a.type !== b.type) return a.type === 'directory' ? -1 : 1;
        return a.name.localeCompare(b.name);
      }));
    } catch (reason) {
      if (!controller.signal.aborted) setError(reason instanceof Error ? reason.message : '无法读取目录');
    } finally {
      if (!controller.signal.aborted) setLoading(false);
    }
  }, [rootDir]);

  useEffect(() => {
    setEntries([]);
    setError(null);
  }, [rootDir]);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => () => requestRef.current?.abort(), []);

  if (!rootDir) {
    return <div className="studio-tree-empty text-xs p-4 text-center">选择一个文件夹开始</div>;
  }

  const visible = filterHidden(entries, showHidden);

  return (
    <div className="studio-tree overflow-y-auto flex-1 py-1">
      {error && (
        <div className="studio-tree-status flex items-center gap-2 px-3 py-2 text-xs text-red-500">
          <span className="flex-1">{error}</span>
          <button type="button" className="underline" onClick={() => { void load(); }}>重试</button>
        </div>
      )}
      {loading && entries.length === 0 && <div className="studio-tree-status p-4 text-center text-xs">加载中...</div>}
      {!loading && !error && visible.length === 0 && <div className="studio-tree-empty p-4 text-center text-xs">文件夹为空</div>}
      {visible.map(entry => (
        <TreeNode
          key={entry.name}
          entry={entry}
          path={rootDir}
          depth={0}
          showHidden={showHidden}
          onOpenFile={onOpenFile}
          onRefresh={load}
        />
      ))}
    </div>
  );
}
