import { useEffect, useMemo, useState, useRef } from 'react';
import { File as FileIcon, Folder } from 'lucide-react';
import { apiFetch } from '@/core/utils/apiFetch';
import { useShowHiddenFiles, filterHidden } from '@/core/utils/showHiddenFiles';
import type { ChatAttachment } from '../../types';

interface FsEntry { name: string; type: 'file' | 'directory'; size?: number; full: string }

interface Props {
  query: string;
  onPick: (attachment: ChatAttachment) => void;
  /** 进入目录时回调：把 @ 后的路径替换为新路径（继续下钻）*/
  onNavigate?: (newQuery: string) => void;
  onCancel: () => void;
  /** 会话级工作目录。设置后 @ 空查询时自动列出 cwd 下的文件。 */
  cwd?: string;
}

export default function AtMentionPicker({ query, onPick, onNavigate, onCancel, cwd }: Props) {
  const [activeIdx, setActiveIdx] = useState(0);
  const [entries, setEntries] = useState<FsEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const showHidden = useShowHiddenFiles();

  // 选中项滚进可视区
  const activeRef = useRef<HTMLButtonElement>(null);

  // 拆成 目录 + 文件名前缀。query 不含 / 时默认列 cwd（无 cwd 则根 /）
  const lastSlash = query.lastIndexOf('/');
  const dir = !query.includes('/') ? (cwd || '/') : (query.slice(0, lastSlash) || '/');
  const namePrefix = !query.includes('/') ? query.toLowerCase() : query.slice(lastSlash + 1).toLowerCase();

  // ---- 单层列目录（防抖）。用 /list 而非递归 search，秒回 ----
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(async () => {
      setLoading(true);
      try {
        const data = await apiFetch<{ success?: boolean; path?: string; entries?: Array<{ name: string; type: 'file' | 'directory'; size?: number }> }>(`/api/fs/list?path=${encodeURIComponent(dir)}`);
        const baseDir = data.path || dir;
        const list: FsEntry[] = (data.success && Array.isArray(data.entries) ? data.entries : []).map(
          (e: { name: string; type: 'file' | 'directory'; size?: number }) => ({
            ...e,
            full: (baseDir.endsWith('/') ? baseDir : baseDir + '/') + e.name,
          }),
        );
        setEntries(list);
      } catch {
        setEntries([]);
      } finally {
        setLoading(false);
      }
    }, 200);
    return () => { if (debounceRef.current) clearTimeout(debounceRef.current); };
  }, [dir]);

  // 前缀过滤（前端，即时）：目录排前，文件在后
  const fileResults = useMemo(() => {
    const visible = filterHidden(entries, showHidden);
    const matched = namePrefix
      ? visible.filter(e => e.name.toLowerCase().startsWith(namePrefix))
      : visible;
    return [...matched]
      .sort((a, b) => (a.type === b.type ? a.name.localeCompare(b.name) : a.type === 'directory' ? -1 : 1))
      .slice(0, 30);
  }, [entries, namePrefix, showHidden]);

  useEffect(() => { setActiveIdx(0); }, [query]);
  useEffect(() => { activeRef.current?.scrollIntoView({ block: 'nearest' }); }, [activeIdx]);

  const chooseEntry = (entry: FsEntry) => {
    if (entry.type === 'directory') {
      onNavigate?.(entry.full + '/');
      return;
    }
    onPick({
      type: 'file', id: entry.full, title: entry.name,
      path: entry.full, content: '', size: entry.size,
    });
  };

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const handlesKey = e.key === 'Escape'
        || e.key === 'ArrowDown'
        || e.key === 'ArrowUp'
        || (e.key === 'Enter' && !e.shiftKey);
      if (!handlesKey) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape') { onCancel(); }
      else if (e.key === 'ArrowDown') { setActiveIdx(i => Math.min(fileResults.length - 1, i + 1)); }
      else if (e.key === 'ArrowUp') { setActiveIdx(i => Math.max(0, i - 1)); }
      else if (e.key === 'Enter' && fileResults.length > 0) {
        chooseEntry(fileResults[activeIdx]);
      }
    };
    window.addEventListener('keydown', handler, true);
    return () => window.removeEventListener('keydown', handler, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fileResults, activeIdx, onCancel]);

  return (
    <div className="precision-popover precision-command-picker model-selector-dropdown p-2 max-h-[280px] overflow-y-auto" onMouseDown={e => e.preventDefault()}>
      <div className="precision-picker-label text-[10px] uppercase tracking-[0.16em] px-2 py-1 font-semibold flex items-center gap-1.5">
        本地文件 · ↑↓ Enter 选择 · Esc 取消 · 选目录可下钻
      </div>

      {loading ? (
        <div className="precision-empty-copy p-4 text-center text-xs">读取目录中…</div>
      ) : fileResults.length === 0 ? (
        <div className="precision-empty-copy p-4 text-center text-xs">该目录无匹配项</div>
      ) : (
        <div className="space-y-0.5">
          {fileResults.map((e, i) => (
            <button
              key={e.full}
              ref={i === activeIdx ? activeRef : undefined}
              onClick={() => chooseEntry(e)}
              onMouseEnter={() => setActiveIdx(i)}
              className={`precision-picker-option w-full text-left px-3 py-2 flex items-center gap-2.5 ${
                i === activeIdx ? 'is-active' : ''
              }`}
            >
              {e.type === 'directory'
                ? <Folder size={13} className="precision-picker-icon flex-shrink-0" />
                : <FileIcon size={13} className="precision-picker-icon flex-shrink-0" />}
              <div className="flex-1 min-w-0">
                <div className="precision-option-title text-sm font-medium truncate">
                  {e.name}{e.type === 'directory' ? '/' : ''}
                </div>
              </div>
              {e.type === 'file' && e.size != null && (
                <span className="precision-message-meta text-[10px] flex-shrink-0">{e.size < 1024 ? `${e.size}B` : `${(e.size / 1024).toFixed(0)}KB`}</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
