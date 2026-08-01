import { useRef, useState } from 'react';
import { ArrowDown, ArrowUp, Check, CircleAlert, CornerDownRight, MoreHorizontal, Play, SkipForward, Trash2, X } from 'lucide-react';
import { showToast } from '@/shell';
import type { QueuedMessage, QueuedMessageDelivery, SessionRuntime } from '../../types';

interface Props {
  runtime: SessionRuntime;
  onUpdate: (itemId: string, patch: { content?: string; position?: number; delivery?: QueuedMessageDelivery }) => Promise<void>;
  onDelete: (itemId: string) => Promise<void>;
  onControl: (action: 'resume' | 'send_next' | 'clear_queue') => Promise<void>;
}

const statusLabel: Record<QueuedMessage['status'], string> = {
  queued: '排队中',
  waiting_safe_point: '等待安全节点',
  paused: '已暂停',
  claimed: '正在发送',
  failed: '发送失败',
};

export default function QueueDock({ runtime, onUpdate, onDelete, onControl }: Props) {
  const [menuId, setMenuId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const pendingActionRef = useRef(false);
  if (runtime.queue.length === 0) return null;

  const editable = (item: QueuedMessage) => item.status !== 'claimed' && item.source !== 'runtime_callback';
  const runAction = async (key: string, action: () => Promise<void>, successMessage?: string) => {
    if (pendingActionRef.current) return;
    pendingActionRef.current = true;
    setPendingAction(key);
    try {
      await action();
      if (successMessage) showToast({ message: successMessage, type: 'info' });
    } catch (reason) {
      showToast({ message: reason instanceof Error ? reason.message : '队列操作失败', type: 'error' });
    } finally {
      pendingActionRef.current = false;
      setPendingAction(null);
    }
  };

  return (
    <section className="chat-queue-dock" aria-label="待发送消息队列">
      {runtime.paused && (
        <header>
          <span>队列已暂停 · 还有 {runtime.queue.length} 条</span>
          <div>
            <button type="button" disabled={pendingAction !== null} onClick={() => void runAction('send_next', () => onControl('send_next'))}><SkipForward size={13} />发送下一条</button>
            <button type="button" disabled={pendingAction !== null} onClick={() => void runAction('resume', () => onControl('resume'))}><Play size={13} />继续队列</button>
            <button type="button" disabled={pendingAction !== null} onClick={() => void runAction('clear_queue', () => onControl('clear_queue'))}>清空</button>
          </div>
        </header>
      )}
      <div className="chat-queue-list">
        {runtime.queue.map((item, index) => {
          const isEditing = editingId === item.id;
          const menuOpen = menuId === item.id;
          const runtimeCallback = item.source === 'runtime_callback';
          return (
            <div key={item.id} className="chat-queue-row" data-status={item.status} data-source={item.source ?? 'user'}>
              {runtimeCallback
                ? <CircleAlert size={15} className="chat-queue-icon" />
                : <CornerDownRight size={15} className="chat-queue-icon" />}
              {isEditing ? (
                <div className="chat-queue-editor">
                  <input value={draft} onChange={event => setDraft(event.target.value)} onKeyDown={event => {
                    if (event.key === 'Escape') setEditingId(null);
                    if (event.key === 'Enter' && draft.trim()) {
                      void runAction(`edit:${item.id}`, () => onUpdate(item.id, { content: draft.trim() }))
                        .then(() => setEditingId(null));
                    }
                  }} autoFocus />
                  <button type="button" aria-label="保存编辑" disabled={!draft.trim() || pendingAction !== null} onClick={() => void runAction(`edit:${item.id}`, () => onUpdate(item.id, { content: draft.trim() })).then(() => setEditingId(null))}><Check size={13} /></button>
                  <button type="button" aria-label="取消编辑" onClick={() => setEditingId(null)}><X size={13} /></button>
                </div>
              ) : (
                <>
                  {runtimeCallback ? (
                    <div className="chat-queue-copy is-system" title={item.message.content}>
                      <span>协作者结果待处理</span>
                      <small>{statusLabel[item.status]}</small>
                    </div>
                  ) : (
                    <>
                      <button type="button" className="chat-queue-copy" disabled={!editable(item)} onClick={() => {
                        setDraft(item.message.content);
                        setEditingId(item.id);
                      }} title={item.message.content}>
                        <span>{item.message.content}</span>
                        <small>{statusLabel[item.status]}</small>
                      </button>
                      <button
                        type="button"
                        className="chat-queue-guide"
                        disabled={pendingAction !== null || !editable(item) || !runtime.activeTurnId || item.delivery === 'guidance'}
                        title={!runtime.activeTurnId ? '当前没有运行中的任务' : '在下一个安全节点引导当前任务'}
                        onClick={() => void runAction(`guide:${item.id}`, () => onUpdate(item.id, { delivery: 'guidance' }))}
                      ><CornerDownRight size={13} />引导</button>
                      <button type="button" className="chat-queue-delete" disabled={pendingAction !== null || !editable(item)} aria-label="从队列删除" onClick={() => {
                        void runAction(`delete:${item.id}`, () => onDelete(item.id), '已从队列移除');
                      }}><Trash2 size={14} /></button>
                      <div className="chat-queue-more">
                        <button type="button" disabled={!editable(item)} aria-label="更多队列操作" onClick={() => setMenuId(menuOpen ? null : item.id)}><MoreHorizontal size={15} /></button>
                        {menuOpen && (
                          <div className="precision-popover">
                            <button type="button" onClick={() => { setDraft(item.message.content); setEditingId(item.id); setMenuId(null); }}>编辑</button>
                            <button type="button" disabled={pendingAction !== null || index === 0} onClick={() => { void runAction(`up:${item.id}`, () => onUpdate(item.id, { position: index - 1 })); setMenuId(null); }}><ArrowUp size={12} />上移</button>
                            <button type="button" disabled={pendingAction !== null || index === runtime.queue.length - 1} onClick={() => { void runAction(`down:${item.id}`, () => onUpdate(item.id, { position: index + 1 })); setMenuId(null); }}><ArrowDown size={12} />下移</button>
                          </div>
                        )}
                      </div>
                    </>
                  )}
                </>
              )}
            </div>
          );
        })}
      </div>
    </section>
  );
}
