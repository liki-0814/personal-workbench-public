import { useState, useMemo, useCallback } from 'react';
import {
  Clipboard,
  Copy,
  Trash2,
  Link,
  Code,
  Search,
  X,
  FileText,
  Image,
  Columns3,
  Clock3,
} from 'lucide-react';
import { showToast } from '@/shell';
import type { ClipboardItem, ClipboardItemType, ToolboxTool } from '@/domain/toolbox';
import { useToolboxState } from '../../state/toolboxState';
import DiffMergeTool from '../tools/DiffMergeTool';
import TimestampTool from '../tools/TimestampTool';
import '../toolbox.css';

interface Props {
  history: ClipboardItem[];
  onAdd: (content: string) => void;
  onRemove: (id: string) => void;
  onClear: () => void;
  onCopy: (content: string, type?: ClipboardItemType) => Promise<boolean>;
  onCopyCallback?: (content: string) => void;
}

type FilterType = 'all' | ClipboardItemType;

const typeConfig: Record<ClipboardItemType, { icon: typeof FileText; label: string; color: string }> = {
  text: { icon: FileText, label: '文本', color: 'text-gray-500' },
  url: { icon: Link, label: '链接', color: 'text-blue-500' },
  code: { icon: Code, label: '代码', color: 'text-gray-500' },
  image: { icon: Image, label: '图片', color: 'text-gray-500' },
};

const filterTabs: { key: FilterType; label: string }[] = [
  { key: 'all', label: '全部' },
  { key: 'text', label: '文本' },
  { key: 'url', label: '链接' },
  { key: 'code', label: '代码' },
  { key: 'image', label: '图片' },
];

function formatTime(iso: string): string {
  const d = new Date(iso);
  const now = new Date();
  const isToday = d.toDateString() === now.toDateString();
  const hours = String(d.getHours()).padStart(2, '0');
  const minutes = String(d.getMinutes()).padStart(2, '0');
  if (isToday) {
    return `${hours}:${minutes}`;
  }
  const month = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${month}-${day} ${hours}:${minutes}`;
}

function truncate(str: string, max: number): string {
  if (str.length <= max) return str;
  return str.slice(0, max) + '...';
}

export default function ClipboardPanel({
  history,
  onAdd,
  onRemove,
  onClear,
  onCopy,
  onCopyCallback,
}: Props) {
  const [filter, setFilter] = useState<FilterType>('all');
  const [search, setSearch] = useState('');
  const [showConfirmClear, setShowConfirmClear] = useState(false);
  const [manualInput, setManualInput] = useState('');
  const [showManualInput, setShowManualInput] = useState(false);
  const [previewImage, setPreviewImage] = useState<string | null>(null);
  const { state: toolboxState, update: updateToolbox } = useToolboxState();
  const activeTool = toolboxState.activeTool;

  const onToolboxChange = useCallback((next: typeof toolboxState) => {
    updateToolbox(() => next);
  }, [updateToolbox]);

  const setActiveTool = useCallback((tool: ToolboxTool) => {
    onToolboxChange({ ...toolboxState, activeTool: tool });
  }, [onToolboxChange, toolboxState]);

  const filtered = useMemo(() => {
    let items = history;
    if (filter !== 'all') {
      items = items.filter(item => item.type === filter);
    }
    if (search.trim()) {
      const q = search.trim().toLowerCase();
      items = items.filter(item =>
        item.type === 'image'
          ? '图片'.includes(q) || formatTime(item.timestamp).includes(q)
          : item.content.toLowerCase().includes(q)
      );
    }
    return items;
  }, [history, filter, search]);

  const handleCopy = useCallback(
    async (item: ClipboardItem) => {
      const success = await onCopy(item.content, item.type);
      if (success) {
        showToast({ message: '已复制到剪贴板', type: 'success' });
        onCopyCallback?.(item.content);
      } else {
        showToast({ message: '复制失败', type: 'error' });
      }
    },
    [onCopy, onCopyCallback]
  );

  const handleManualAdd = useCallback(() => {
    if (!manualInput.trim()) return;
    onAdd(manualInput.trim());
    setManualInput('');
    setShowManualInput(false);
    showToast({ message: '已添加到剪贴板历史', type: 'success' });
  }, [manualInput, onAdd]);

  const handleClear = useCallback(() => {
    onClear();
    setShowConfirmClear(false);
    showToast({ message: '剪贴板历史已清空', type: 'info' });
  }, [onClear]);

  const handlePasteImage = useCallback(async () => {
    try {
      const items = await navigator.clipboard.read();
      for (const clipboardItem of items) {
        const imageType = clipboardItem.types.find(t => t.startsWith('image/'));
        if (imageType) {
          const blob = await clipboardItem.getType(imageType);
          const reader = new FileReader();
          reader.onloadend = () => {
            const base64 = reader.result as string;
            onAdd(base64);
            showToast({ message: '图片已添加到剪贴板历史', type: 'success' });
          };
          reader.readAsDataURL(blob);
          return;
        }
      }
      showToast({ message: '剪贴板中没有图片', type: 'error' });
    } catch {
      showToast({ message: '读取剪贴板失败，请检查权限', type: 'error' });
    }
  }, [onAdd]);

  return (
    <div className="clipboard-panel flex flex-col h-full">
      {/* Tool tabs */}
      <div className="toolbox-tool-tabs flex gap-1 mb-3 flex-wrap" role="tablist" aria-label="工具">
        <button
          type="button"
          role="tab"
          aria-selected={activeTool === 'clipboard'}
          onClick={() => setActiveTool('clipboard')}
          className={`toolbox-tool-tab flex items-center gap-1.5 px-3 py-2 text-sm font-medium transition-colors ${
            activeTool === 'clipboard'
              ? 'small-btn-active'
              : 'small-btn'
          }`}
        >
          <Clipboard size={14} />
          剪贴板
          <span className="count-badge">{history.length}</span>
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={activeTool === 'diff'}
          onClick={() => setActiveTool('diff')}
          className={`toolbox-tool-tab flex items-center gap-1.5 px-3 py-2 text-sm font-medium transition-colors ${activeTool === 'diff' ? 'small-btn-active' : 'small-btn'}`}
        >
          <Columns3 size={14} />
          Diff 与合并
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={activeTool === 'timestamp'}
          onClick={() => setActiveTool('timestamp')}
          className={`toolbox-tool-tab flex items-center gap-1.5 px-3 py-2 text-sm font-medium transition-colors ${activeTool === 'timestamp' ? 'small-btn-active' : 'small-btn'}`}
        >
          <Clock3 size={14} />
          时间戳转换
        </button>
      </div>

      <div className="toolbox-tool-stage" role="tabpanel">

      {activeTool === 'diff' && (
        <div className="toolbox-tool-body flex-1 overflow-y-auto min-h-0">
          <DiffMergeTool
            state={toolboxState.diff}
            onChange={diff => onToolboxChange({ ...toolboxState, diff })}
          />
        </div>
      )}

      {activeTool === 'timestamp' && (
        <div className="toolbox-tool-body flex-1 overflow-y-auto min-h-0">
          <TimestampTool
            state={toolboxState.timestamp}
            onChange={timestamp => onToolboxChange({ ...toolboxState, timestamp })}
          />
        </div>
      )}

      {activeTool === 'clipboard' && (
        <>
          {/* Actions bar */}
          <div className="toolbox-actions flex items-center gap-2 mb-3">
            <button
              onClick={() => setShowManualInput(v => !v)}
              className="small-btn"
              title="手动添加文本"
            >
              <FileText size={14} />
              <span className="hidden sm:inline">添加文本</span>
            </button>
            <button
              onClick={handlePasteImage}
              className="small-btn"
              title="从剪贴板粘贴图片"
            >
              <Image size={14} />
              <span className="hidden sm:inline">粘贴图片</span>
            </button>
            {history.length > 0 && (
              <button
                onClick={() => setShowConfirmClear(true)}
                className="small-btn text-red-500 hover:text-red-600"
                title="清空全部"
              >
                <Trash2 size={14} />
              </button>
            )}
          </div>

          {/* Manual input */}
          {showManualInput && (
            <div className="toolbox-inline modal-inline mb-4 animate-fade-in-up">
              <textarea
                value={manualInput}
                onChange={e => setManualInput(e.target.value)}
                placeholder="粘贴内容到此处..."
                className="input-field w-full min-h-[60px] resize-none"
                rows={2}
                autoFocus
              />
              <div className="flex gap-2 justify-end mt-2">
                <button
                  onClick={() => {
                    setShowManualInput(false);
                    setManualInput('');
                  }}
                  className="cancel-btn"
                >
                  取消
                </button>
                <button onClick={handleManualAdd} className="confirm-btn">
                  添加
                </button>
              </div>
            </div>
          )}

          {/* Search */}
          <div className="toolbox-search relative mb-3">
            <Search
              size={14}
              className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400 pointer-events-none"
            />
            <input
              value={search}
              onChange={e => setSearch(e.target.value)}
              placeholder="搜索剪贴板内容..."
              className="input-field w-full h-10"
              style={{ paddingLeft: '36px', paddingRight: '32px' }}
            />
            {search && (
              <button
                onClick={() => setSearch('')}
                className="absolute right-2 top-1/2 -translate-y-1/2 p-0.5 rounded text-gray-400 hover:text-gray-600 transition-colors"
              >
                <X size={14} />
              </button>
            )}
          </div>

          {/* Filter tabs */}
          <div className="toolbox-filter-tabs flex gap-0 mb-3">
            {filterTabs.map(tab => (
              <button
                key={tab.key}
                onClick={() => setFilter(tab.key)}
                className={`toolbox-filter-tab flex-1 flex items-center justify-center gap-1 px-2 py-1.5 text-xs font-medium transition-colors ${
                  filter === tab.key
                    ? 'small-btn-active'
                    : 'small-btn'
                }`}
              >
                <span className="whitespace-nowrap">{tab.label}</span>
                <span className="count-badge flex-shrink-0">
                  {tab.key === 'all'
                    ? history.length
                    : history.filter(i => i.type === tab.key).length}
                </span>
              </button>
            ))}
          </div>

          {/* List */}
          <div className="toolbox-list flex-1 overflow-y-auto min-h-0 pr-1">
            {filtered.map(item => {
              const config = typeConfig[item.type];
              const Icon = config.icon;
              return (
                <div
                  key={item.id}
                  className={`clipboard-item group ${
                    item.type === 'code'
                      ? 'clipboard-item-code'
                      : item.type === 'url'
                        ? 'clipboard-item-url'
                        : ''
                  }`}
                >
                  <div className="flex items-start gap-3">
                    <div
                      className={`flex-shrink-0 mt-0.5 ${config.color}`}
                    >
                      <Icon size={16} />
                    </div>
                    <div className="flex-1 min-w-0">
                      {item.type === 'image' ? (
                        <div className="relative">
                          <img
                            src={item.content}
                            alt="剪贴板图片"
                            className="max-h-32 rounded-lg object-contain cursor-pointer hover:opacity-90 transition-opacity"
                            onClick={() => setPreviewImage(item.content)}
                          />
                        </div>
                      ) : (
                        <p
                          className={`text-sm leading-relaxed break-all ${
                            item.type === 'url'
                              ? 'text-blue-600 dark:text-blue-400'
                              : 'text-gray-700 dark:text-gray-200'
                          }`}
                        >
                          {truncate(item.content, 80)}
                        </p>
                      )}
                      <span className="text-[10px] text-gray-400 mt-1 block">
                        {formatTime(item.timestamp)}
                        {item.source ? ` · ${item.source}` : ''}
                      </span>
                    </div>
                    <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0">
                      <button
                        onClick={e => {
                          e.stopPropagation();
                          handleCopy(item);
                        }}
                        className="toolbox-row-action p-1.5 text-gray-400 transition-colors"
                        title="复制"
                      >
                        <Copy size={14} />
                      </button>
                      <button
                        onClick={e => {
                          e.stopPropagation();
                          onRemove(item.id);
                        }}
                        className="toolbox-row-action toolbox-row-action-danger p-1.5 text-gray-400 transition-colors"
                        title="删除"
                      >
                        <Trash2 size={14} />
                      </button>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>

          {/* Empty state */}
          {filtered.length === 0 && (
            <div className="empty-state animate-fade-in-up">
              <div className="toolbox-empty-mark w-10 h-10 flex items-center justify-center mb-3">
                <Clipboard size={20} />
              </div>
              <p className="text-sm text-gray-500 dark:text-gray-400">
                {history.length === 0 ? '还没有剪贴板记录' : '没有匹配的记录'}
              </p>
              {history.length === 0 && (
                <div className="flex gap-2 mt-3">
                  <button
                    onClick={() => setShowManualInput(true)}
                    className="text-teal-500 text-sm hover:text-teal-400 font-medium"
                  >
                    手动添加
                  </button>
                  <span className="text-gray-300">|</span>
                  <button
                    onClick={handlePasteImage}
                    className="text-teal-500 text-sm hover:text-teal-400 font-medium"
                  >
                    粘贴图片
                  </button>
                </div>
              )}
            </div>
          )}

          {/* Clear confirmation */}
          {showConfirmClear && (
            <div className="toolbox-inline modal-inline mt-4 animate-fade-in-up">
              <p className="text-sm text-gray-600 dark:text-gray-300 mb-3">
                确定要清空全部 {history.length} 条剪贴板记录吗？此操作不可撤销。
              </p>
              <div className="flex gap-2 justify-end">
                <button
                  onClick={() => setShowConfirmClear(false)}
                  className="cancel-btn"
                >
                  取消
                </button>
                <button onClick={handleClear} className="confirm-btn">
                  确认清空
                </button>
              </div>
            </div>
          )}
        </>
      )}
      </div>

      {/* Image preview modal */}
      {previewImage && (
        <div
          className="toolbox-overlay fixed inset-0 z-50 flex items-center justify-center animate-modal-fade"
          style={{ background: 'rgba(0,0,0,0.6)' }}
          onClick={() => setPreviewImage(null)}
        >
          <div className="relative max-w-[90vw] max-h-[90vh]" onClick={e => e.stopPropagation()}>
            <img
              src={previewImage}
              alt="预览"
              className="toolbox-preview-image max-w-full max-h-[90vh]"
            />
            <button
              onClick={() => setPreviewImage(null)}
              className="toolbox-preview-close absolute -top-3 -right-3 w-8 h-8 text-gray-600 dark:text-gray-300 flex items-center justify-center transition-colors"
            >
              <X size={16} />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
