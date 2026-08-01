import React, { useRef, useEffect, useState, useCallback } from 'react';
import { Send, Square, Loader2, X, ImageIcon, File as FileIcon, FolderOpen, Monitor } from 'lucide-react';
import { SkillSlashPicker } from '../shared';
import { useAgentSkills } from '../../state/agentStore';
import type { ChatAttachment } from '../../types';
import type { ChatMode } from '@/domain/chat';
import AtMentionPicker from '../shared/AtMentionPicker';
import { FilePromptPicker } from '@/domain/studio';
import {
  estimateDraftExtrasTokens,
  estimateDraftTokensWithExtras,
  formatTokenAmount,
  getContextUsagePercent,
  type ContextWindowUsage,
} from '../../utils/contextWindow';

export interface ChatInputCardProps {
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void | Promise<void>;
  onGuide?: () => void | Promise<void>;
  disabled?: boolean;
  placeholder?: string;
  images?: string[];
  onImagesChange?: (images: string[]) => void;
  streaming?: boolean;
  onStop?: () => void | Promise<void>;
  /** Hide the image button entirely (e.g. Wiki assistant). */
  hideImages?: boolean;
  /** Whether the current model can accept image input. Defaults to true. */
  acceptsImages?: boolean;
  imageDisabledReason?: string;
  /** 附件状态 */
  attachments?: ChatAttachment[];
  onAttachmentsChange?: (attachments: ChatAttachment[]) => void;
  /** 空输入框按 ↑ 时调出的上一条用户消息文本 */
  lastUserText?: string;
  /** 当前会话模式，direct 模式下隐藏 / skill picker */
  sessionMode?: ChatMode;
  /** 会话级工作目录，@ 选择器默认从此目录浏览 */
  cwd?: string;
  /** 可见会话的上下文估算；组件会再叠加当前未发送输入。 */
  contextUsage?: ContextWindowUsage;
  onOpenWorkspace?: () => void;
}

export default function ChatInputCard({ value, onChange, onSubmit, onGuide, disabled, placeholder, images = [], onImagesChange, streaming, onStop, hideImages, acceptsImages = true, imageDisabledReason, attachments = [], onAttachmentsChange, lastUserText, sessionMode = 'agent', cwd, contextUsage, onOpenWorkspace }: ChatInputCardProps) {
  // `/` → skill picker (agent skills);  `>` → file prompt picker
  const skills = useAgentSkills();
  const skillOpen = sessionMode === 'agent' && value.startsWith('/');
  const promptOpen = value.startsWith('>');
  const [promptsDir, setPromptsDir] = useState('');
  useEffect(() => {
    fetch('/api/agent/config/output')
      .then(r => r.ok ? r.json() : null)
      .then(d => { if (d?.prompts_dir) setPromptsDir(d.prompts_dir); })
      .catch(() => {});
  }, []);

  // @ mention detection — 仅用于文件引用
  const [atQuery, setAtQuery] = useState<string | null>(null);
  const atOpen = atQuery !== null;

  const handleAtChange = (newValue: string) => {
    const cursorPos = textareaRef.current?.selectionStart ?? newValue.length;
    onChange(newValue);
    const textBeforeCursor = newValue.slice(0, cursorPos);
    const atMatch = textBeforeCursor.match(/@([^\s@]*)$/);
    if (atMatch && (atMatch.index === 0 || /[\s\n]/.test(newValue[atMatch.index! - 1]))) {
      setAtQuery(atMatch[1]);
    } else {
      setAtQuery(null);
    }
  };

  const handleAtNavigate = (newPath: string) => {
    const cursorPos = textareaRef.current?.selectionStart ?? value.length;
    const before = value.slice(0, cursorPos);
    const after = value.slice(cursorPos);
    const newBefore = before.replace(/@[^\s@]*$/, '@' + newPath);
    onChange(newBefore + after);
    setAtQuery(newPath);
    const newCursor = newBefore.length;
    setTimeout(() => {
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(newCursor, newCursor);
    }, 0);
  };

  const handleAtPick = (attachment: ChatAttachment) => {
    const cursorPos = textareaRef.current?.selectionStart ?? value.length;
    const before = value.slice(0, cursorPos);
    const after = value.slice(cursorPos);
    const cleanedBefore = before.replace(/@[^\s@]*$/, '');
    const next = cleanedBefore + after;
    onChange(next);
    setAtQuery(null);
    if (onAttachmentsChange && !attachments.some(a => a.id === attachment.id)) {
      onAttachmentsChange([...attachments, attachment]);
    }
    setTimeout(() => {
      textareaRef.current?.setSelectionRange(cleanedBefore.length, cleanedBefore.length);
    }, 0);
  };
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [stopping, setStopping] = useState(false);
  const stoppingRef = useRef(false);
  const [submitting, setSubmitting] = useState(false);
  const submittingRef = useRef(false);
  const draftExtrasTokens = React.useMemo(
    () => estimateDraftExtrasTokens(images, attachments),
    [images, attachments],
  );
  const draftTokens = estimateDraftTokensWithExtras(value, draftExtrasTokens);
  const usedTokens = contextUsage
    ? contextUsage.usedTokens + draftTokens
    : 0;
  const usagePercent = contextUsage
    ? getContextUsagePercent(usedTokens, contextUsage.windowTokens)
    : 0;
  const usageTone = usagePercent >= 85 ? 'danger' : usagePercent >= 65 ? 'warning' : 'normal';
  useEffect(() => {
    if (!streaming) setStopping(false);
  }, [streaming]);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // picker（skill / prompt / @）打开期间，导航键交给 picker 处理
    const pickerKey = e.key === 'ArrowDown'
      || e.key === 'ArrowUp'
      || e.key === 'Escape'
      || (e.key === 'Enter' && !e.shiftKey);
    if ((skillOpen || promptOpen || atOpen) && pickerKey) {
      return;
    }
    // Esc：生成中则停止；否则不拦截（让其他浮层处理）
    if (e.key === 'Escape' && streaming && onStop) {
      e.preventDefault();
      void runStop();
      return;
    }
    // 空输入框按 ↑：调出上一条用户消息编辑（类似终端 / Slack）
    if (e.key === 'ArrowUp' && !value && lastUserText && !e.nativeEvent.isComposing) {
      e.preventDefault();
      onChange(lastUserText);
      return;
    }
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      if (disabled || submitting || (!value.trim() && images.length === 0 && attachments.length === 0)) return;
      if ((e.metaKey || e.ctrlKey) && streaming && onGuide) void runSubmission(onGuide);
      else void runSubmission(onSubmit);
    }
  };

  const runStop = async () => {
    if (!onStop || stoppingRef.current) return;
    stoppingRef.current = true;
    setStopping(true);
    try {
      await onStop();
    } catch {
      // useAiChat exposes the daemon error through the stream error state.
    } finally {
      stoppingRef.current = false;
      setStopping(false);
    }
  };

  const runSubmission = async (action: () => void | Promise<void>) => {
    if (submittingRef.current) return;
    submittingRef.current = true;
    setSubmitting(true);
    try {
      await action();
    } finally {
      submittingRef.current = false;
      setSubmitting(false);
    }
  };

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = Math.min(el.scrollHeight, 200) + 'px';
  }, [value]);

  const readImages = useCallback(async (files: File[]) => {
    const encoded = await Promise.all(files.map(file => new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result as string);
      reader.onerror = () => reject(reader.error ?? new Error(`无法读取图片 ${file.name}`));
      reader.readAsDataURL(file);
    })));
    onImagesChange?.([...images, ...encoded]);
  }, [images, onImagesChange]);

  const handlePaste = useCallback((e: React.ClipboardEvent) => {
    const items = e.clipboardData.items;
    let hasImage = false;
    const files: File[] = [];
    for (const item of items) {
      if (item.type.startsWith('image/')) {
        hasImage = true;
        if (!acceptsImages) continue;
        const file = item.getAsFile();
        if (file) files.push(file);
      }
    }
    if (hasImage) {
      e.preventDefault();
      if (!acceptsImages) {
        window.alert('当前模型不支持图片输入，请切换到支持视觉的模型再粘贴图片。');
      } else if (files.length > 0) {
        void readImages(files);
      }
    }
  }, [acceptsImages, readImages]);

  const handleFileSelect = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (!files) return;
    if (!acceptsImages) {
      e.target.value = '';
      window.alert('当前模型不支持图片输入，请切换到支持视觉的模型再上传图片。');
      return;
    }
    void readImages(Array.from(files).filter(file => file.type.startsWith('image/')));
    e.target.value = '';
  }, [acceptsImages, readImages]);

  const removeImage = useCallback((idx: number) => {
    onImagesChange?.(images.filter((_, i) => i !== idx));
  }, [images, onImagesChange]);

  return (
    <div className="relative">
      {skillOpen && (
        <div className="absolute bottom-full left-0 right-0 mb-2 z-30">
          <SkillSlashPicker
            skills={skills}
            query={value}
            onPick={(directive: string) => {
              onChange(directive);
              setTimeout(() => textareaRef.current?.focus(), 0);
            }}
            onCommand={(command) => {
              onChange(command);
              setTimeout(() => textareaRef.current?.focus(), 0);
            }}
            onCancel={() => onChange('')}
          />
        </div>
      )}
      {promptOpen && promptsDir && (
        <div className="absolute bottom-full left-0 right-0 mb-2 z-30">
          <FilePromptPicker
            query={value}
            promptsDir={promptsDir}
            onPick={(content) => {
              onChange(content);
              setTimeout(() => textareaRef.current?.focus(), 0);
            }}
            onClose={() => onChange('')}
          />
        </div>
      )}
      {atOpen && (
        <div className="absolute bottom-full left-0 right-0 mb-2 z-30">
          <AtMentionPicker
            query={atQuery || ''}
            onPick={handleAtPick}
            onNavigate={handleAtNavigate}
            onCancel={() => setAtQuery(null)}
            cwd={cwd}
          />
        </div>
      )}
    <div className={`pwb-input-shell precision-composer ${streaming ? 'is-streaming' : ''}`}>
      {cwd && (
        <div className="precision-workspace-strip">
          <button type="button" onClick={onOpenWorkspace} disabled={!onOpenWorkspace} title={cwd}>
            <FolderOpen size={13} />
            <span>{cwd.split('/').filter(Boolean).pop() || cwd}</span>
          </button>
          <span><Monitor size={12} />本地</span>
        </div>
      )}
      {/* @ 附件 chip */}
      {attachments.length > 0 && (
        <div className="flex gap-1.5 mb-2 flex-wrap">
          {attachments.map(a => (
            <span
              key={a.id}
              className="precision-attachment-chip inline-flex items-center gap-1 text-[11px] px-2 py-0.5"
              data-type={a.type}
              title={a.path || a.title}
            >
              <FileIcon size={10} />
              {a.title}
              <button
                onClick={() => onAttachmentsChange?.(attachments.filter(x => x.id !== a.id))}
                className="precision-chip-remove ml-0.5"
                aria-label={`移除附件 ${a.title}`}
              >×</button>
            </span>
          ))}
        </div>
      )}
      {images.length > 0 && (
        <div className="flex gap-2 mb-2 flex-wrap">
          {images.map((img, idx) => (
            <div key={idx} className="relative group">
              <img src={img} alt="" className="precision-image-preview w-16 h-16 object-cover" />
              <button
                onClick={() => removeImage(idx)}
                className="precision-image-remove absolute -top-1 -right-1 w-5 h-5 flex items-center justify-center text-[10px] opacity-0 group-hover:opacity-100 transition-opacity"
                title="移除"
              >
                <X size={10} />
              </button>
            </div>
          ))}
        </div>
      )}
      <textarea
        ref={textareaRef}
        value={value}
        onChange={e => handleAtChange(e.target.value)}
        onKeyDown={handleKeyDown}
        onPaste={handlePaste}
        placeholder={placeholder || '输入问题 · / 调用 Skill · @ 引用文件'}
        disabled={disabled}
        rows={1}
        className="pwb-input-textarea min-h-[24px] max-h-[220px] px-1 py-1"
      />
      <div className="precision-composer-footer flex items-center justify-between mt-1.5">
        <div className="flex items-center gap-1.5 min-w-0">
          {!hideImages && (
            <>
              <button
                type="button"
                className="ai-input-toolbar-btn disabled:opacity-30 disabled:cursor-not-allowed"
                title={acceptsImages ? '添加图片' : imageDisabledReason || '当前模型不支持图片输入'}
                disabled={!acceptsImages}
                onClick={() => fileInputRef.current?.click()}
              >
                <ImageIcon size={18} />
              </button>
              <input
                ref={fileInputRef}
                type="file"
                accept="image/*"
                multiple
                className="hidden"
                onChange={handleFileSelect}
              />
            </>
          )}
          <span className="precision-input-hint">{streaming ? '↵ 排队 · ⌘↵ 引导 · ⇧↵ 换行' : '↵ 发送 · ⇧↵ 换行'}</span>
          {contextUsage && (
            <span
              className="precision-context-meter"
              data-tone={usageTone}
              title={`${contextUsage.exactUsage ? '最近一次模型调用实际用量' : '可见消息与当前输入的近似值'}${contextUsage.exactWindow ? '' : '；当前模型窗口未收录，使用保守估值'}。`}
              aria-label={`上下文窗口约使用 ${formatTokenAmount(usedTokens)}，共 ${formatTokenAmount(contextUsage.windowTokens)}，占 ${usagePercent.toFixed(1)}%`}
            >
              上下文 {contextUsage.exactUsage ? '' : '≈ '}{formatTokenAmount(usedTokens)} / {formatTokenAmount(contextUsage.windowTokens)} · {usagePercent.toFixed(1)}%
            </span>
          )}
        </div>
        <div className="precision-composer-send-actions">
        {streaming && onStop && (
          <button
            type="button"
            onClick={() => void runStop()}
            disabled={stopping}
            className="pwb-send-btn disabled:opacity-50"
            title="停止生成"
            aria-label="停止生成"
          >
            {stopping ? <Loader2 size={16} className="animate-spin" /> : <Square size={14} fill="currentColor" />}
          </button>
        )}
          <button
            type="button"
            onClick={() => void runSubmission(onSubmit)}
            disabled={disabled || submitting || (!value.trim() && images.length === 0 && attachments.length === 0)}
            className="pwb-send-btn"
            title={submitting ? '正在提交' : '发送消息'}
            aria-label={submitting ? '正在提交消息' : '发送消息'}
          >
            {submitting ? <Loader2 size={16} className="animate-spin" /> : <Send size={16} />}
          </button>
        </div>
      </div>
    </div>
    </div>
  );
}
