import { useState, memo, useMemo } from 'react';
import { Wrench, ChevronDown, ChevronLeft, ChevronRight, RefreshCw, Copy, Check, Brain, Zap, FileCode2, GitBranch } from 'lucide-react';
import MarkdownRenderer from '@/shell/ui/MarkdownRenderer';
import CopyableImage from '@/shell/ui/CopyableImage';
import { getMessageContent, getMessageImages, getMessageImageRecords, getMessageStats } from '@/domain/chat';
import type { ChatMessage, GeneratedImageRecord } from '../../types';
import type { WorkspaceRef } from '@/domain/studio';
import { ThinkingIndicator } from './ThinkingIndicator';
import ToolTraceList from './ToolTraceList';
import DecisionTraceList from './DecisionTraceList';
import { DocumentCard, type DocumentRef } from '@/domain/documents';

interface Props {
  msg: ChatMessage;
  isLastAssistant: boolean;
  loading: boolean;
  expanded: boolean;
  onToggleExpand: () => void;
  copied: boolean;
  onCopy: (text: string) => void;
  onRegenerate?: () => void;
  onSwitchVersion?: (versionIdx: number) => void;
  onOpenWorkspaceRef?: (ref: WorkspaceRef) => void;
  onOpenDocument?: (ref: DocumentRef) => void;
  onReuseGeneratedImage?: (record: GeneratedImageRecord, mode: 'edit' | 'variant') => void;
  /** Compact = panel sizing (smaller paddings/icons/fonts). */
  compact?: boolean;
}

function MessageBubbleInner({
  msg,
  isLastAssistant,
  loading,
  expanded,
  onToggleExpand,
  copied,
  onCopy,
  onRegenerate,
  onSwitchVersion,
  onOpenWorkspaceRef,
  onOpenDocument,
  onReuseGeneratedImage,
  compact = false,
}: Props) {
  const msgContent = useMemo(() => getMessageContent(msg), [msg]);
  const msgImages = useMemo(() => getMessageImages(msg), [msg]);
  const msgStats = useMemo(() => getMessageStats(msg), [msg]);
  const imageRecords = useMemo(() => getMessageImageRecords(msg) ?? [], [msg]);
  const hasToolOutcome = useMemo(
    () => !!msg.toolTrace?.some(t => t.result || t.status === 'backgrounded' || t.status === 'done'),
    [msg.toolTrace],
  );
  const hasBackgroundedTool = useMemo(
    () => !!msg.toolTrace?.some(t => t.status === 'backgrounded'),
    [msg.toolTrace],
  );
  const versions = msg.versions || [];
  const activeV = msg.activeVersion ?? -1;
  const versionCount = versions.length + 1;

  const userImgClass = compact ? 'max-w-[160px] max-h-[120px]' : 'max-w-[220px] max-h-[170px]';
  const userMaxWidth = compact ? 'max-w-[88%]' : 'max-w-[78%]';
  const assistantMaxWidth = 'max-w-full';
  const userPadding = compact ? 'px-3.5 py-2.5 text-sm' : 'px-4 py-2.5 text-base';
  const versionIconSize = compact ? 11 : 12;
  const actionTextSize = compact ? 'text-[10px]' : 'text-[11px]';
  const actionPadding = compact ? 'px-1.5 py-0.5 rounded' : 'px-2 py-1 rounded-md';
  const actionIconSize = compact ? 11 : 12;
  const actionGap = compact ? 'gap-1.5 mt-1' : 'gap-2 mt-1.5';
  const avatarSize = compact ? 'precision-agent-mark precision-agent-mark-sm' : 'precision-agent-mark';

  if (msg.systemNotice) {
    return <div className="precision-system-notice">{msg.content}</div>;
  }

  if (msg.role === 'user') {
    const userImages = [...new Set([...(msg.images ?? []), ...(msg.generatedImages ?? [])])];
    return (
      <div className="precision-user-row flex justify-end items-center gap-1.5 group">
        <div className={`pwb-msg-user ${compact ? 'pwb-msg-user-compact' : ''} ${userMaxWidth} min-w-0 ${userPadding} whitespace-pre-wrap`}>
          <span className="whitespace-pre-wrap break-words">{msg.content}</span>
          {userImages.length > 0 && (
            <div className="flex gap-2 mt-2 flex-wrap">
              {userImages.map((img: string, i: number) => (
                <img
                  key={i}
                  src={img}
                  alt=""
                  className={`${userImgClass} rounded-[10px] object-cover`}
                />
              ))}
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className={`precision-message-row flex gap-3 justify-start group ${loading && isLastAssistant ? 'is-streaming' : ''}`}>
      <div className={`${avatarSize} ${loading && isLastAssistant ? 'is-streaming' : ''}`} aria-hidden>AI</div>
      <div className={`flex flex-col ${assistantMaxWidth} min-w-0 flex-1`}>
        {msg.thinking && msg.thinking.length > 0 && (
          <ThinkingBlock thinking={msg.thinking} streaming={loading && isLastAssistant && !msgContent} />
        )}
        {msg.toolTrace && msg.toolTrace.length > 0 && (
          <ToolTraceList traces={msg.toolTrace} compact={compact} />
        )}
        {msg.decisionTrace && msg.decisionTrace.length > 0 && (
          <DecisionTraceList traces={msg.decisionTrace} />
        )}
        {msg.source === 'background' && (
          <div className="precision-background-label flex items-center gap-1.5 mb-1.5 text-[11px] font-medium">
            <Zap size={12} />
            <span>后台任务</span>
          </div>
        )}
        <div className={`pwb-msg-assistant ${compact ? 'text-sm' : ''}`}>
          {/* Pre-stream waiting indicator: shows when bubble exists but model
           * hasn't streamed any visible content yet. */}
          {loading && isLastAssistant && !msgContent && !msg.thinking && (
            <ThinkingIndicator compact={compact} />
          )}
          {!loading && !msgContent && !msg.thinking && !msgImages?.length && hasBackgroundedTool && (
            <span className="text-xs text-amber-600/90 dark:text-amber-400/90 italic">
              任务已转入后台运行，完成后将自动通知。可在右上角「后台任务」查看进度。
            </span>
          )}
          {!loading && !msgContent && !msg.thinking && !msgImages?.length && !hasToolOutcome && !msg.collaborationEvidence?.length && !msg.collaborationTask && (
            <span className="text-xs text-red-400/80 italic">
              {msg.error ? `⚠ ${msg.error}` : '请求未返回内容'}
            </span>
          )}
          {msg.collaborationTask ? (
            <section className="collaboration-task-card">
              <header>
                <div><GitBranch size={14} /><span><small>协作任务</small><strong>{msg.collaborationTask.title || '多 CLI 协作'}</strong></span></div>
                <small className="collaboration-task-status">{msg.collaborationTask.status === 'completed' ? '已完成' : msg.collaborationTask.status === 'awaiting_review' ? '待验收' : msg.collaborationTask.status === 'failed' ? '失败' : '执行中'}</small>
              </header>
              <div className="collaboration-task-meta">
                {msg.collaborationTask.projectName && <span>{msg.collaborationTask.projectName}</span>}
                {msg.collaborationTask.participants && msg.collaborationTask.participants.length > 0 && <span>{msg.collaborationTask.participants.join(' / ')}</span>}
                {msg.collaborationTask.totalSteps != null && <span>{msg.collaborationTask.completedSteps ?? 0}/{msg.collaborationTask.totalSteps} 步</span>}
              </div>
              {msgContent && (
                <details className="collaboration-task-result">
                  <summary>查看任务结果</summary>
                  <div><MarkdownRenderer content={msgContent} /></div>
                </details>
              )}
              {msg.collaborationEvidence && msg.collaborationEvidence.length > 0 && (
                <div className="collaboration-evidence-list">
                  {msg.collaborationEvidence.map(evidence => (
                    <details key={evidence.id} className="collaboration-evidence-card">
                      <summary>
                        <span>{evidence.status === 'blocked' ? '需要处理' : '阶段完成'}</span>
                        <strong>{evidence.source}</strong>
                      </summary>
                      <p>{evidence.summary}</p>
                    </details>
                  ))}
                </div>
              )}
            </section>
          ) : <MarkdownRenderer content={msgContent} />}
          {onOpenWorkspaceRef && msg.workspaceRefs && msg.workspaceRefs.length > 0 && (
            <div className="workspace-ref-list">
              <strong>改动文件</strong>
              {msg.workspaceRefs.map(ref => (
                <button key={`${ref.workspaceRoot ?? ''}:${ref.path}:${ref.line ?? ''}`} type="button" title={ref.path} onClick={() => onOpenWorkspaceRef(ref)}>
                  <FileCode2 size={12} />
                  <span>{ref.kind === 'diff' ? `${ref.path.split('/').pop() || ref.path} 文件审阅` : ref.path}</span>
                  {ref.line && <small>:{ref.line}</small>}
                  <small>{ref.kind === 'diff' ? 'Diff / 最新' : '查看文件'}</small>
                </button>
              ))}
            </div>
          )}
          {onOpenDocument && msg.documentRefs && msg.documentRefs.length > 0 && (
            <div className="chat-document-list">
              {msg.documentRefs.map(document => <DocumentCard key={document.id} document={document} onOpen={() => onOpenDocument(document)} />)}
            </div>
          )}
          {!msg.collaborationTask && msg.collaborationEvidence && msg.collaborationEvidence.length > 0 && (
            <div className="collaboration-evidence-list">
              {msg.collaborationEvidence.map(evidence => (
                <details key={evidence.id} className="collaboration-evidence-card">
                  <summary>
                    <span>{evidence.status === 'blocked' ? '需要处理' : '协作证据'}</span>
                    <strong>{evidence.source}</strong>
                  </summary>
                  <p>{evidence.summary}</p>
                </details>
              ))}
            </div>
          )}
          {loading && isLastAssistant && msgContent && (
            <span className="pwb-caret" aria-hidden />
          )}
          {msgImages && msgImages.length > 0 && (
            <div className="flex gap-2 mt-3 flex-wrap">
              {msgImages.map((img, i) => {
                const record = imageRecords.find(item => item.url === img) ?? imageRecords[i];
                return (
                  <div key={`${img}:${i}`} className="max-w-[640px] min-w-0">
                    <CopyableImage src={img} alt="" compact={compact} />
                    {record && (
                      <div className="mt-2 rounded-xl border border-black/10 bg-black/[0.02] px-3 py-2 text-xs dark:border-white/10 dark:bg-white/[0.03]">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className={record.status === 'ready' ? 'text-emerald-600 dark:text-emerald-400' : 'text-amber-600 dark:text-amber-400'}>
                            {record.qa.status === 'passed' ? '已自动验收' : record.qa.status === 'repaired' ? '已自动修复并验收' : record.qa.status === 'not-checked' ? '未自动验收' : '有验收提示'}
                          </span>
                          <span className="text-black/50 dark:text-white/50">{record.providerName} · {record.aspectRatio}</span>
                          {onReuseGeneratedImage && (
                            <span className="ml-auto flex gap-1.5">
                              <button type="button" className="precision-secondary-button" onClick={() => onReuseGeneratedImage(record, 'edit')}>基于此图修改</button>
                              <button type="button" className="precision-secondary-button" onClick={() => onReuseGeneratedImage(record, 'variant')}>生成相似版本</button>
                            </span>
                          )}
                        </div>
                        {record.qa.issues.length > 0 && (
                          <ul className="mt-2 list-disc space-y-1 pl-4 text-amber-700 dark:text-amber-300">
                            {record.qa.issues.map((issue, issueIndex) => <li key={`${issue.category}:${issueIndex}`}>{issue.description}</li>)}
                          </ul>
                        )}
                        <details className="mt-2 text-black/65 dark:text-white/65">
                          <summary className="cursor-pointer select-none">生成详情</summary>
                          <dl className="mt-2 grid gap-1.5 break-words">
                            <div><dt className="font-medium">场景</dt><dd>{record.scene} / {record.intent}</dd></div>
                            <div><dt className="font-medium">Visual Brief</dt><dd><pre className="mt-1 whitespace-pre-wrap">{JSON.stringify(record.visualBrief, null, 2)}</pre></dd></div>
                            <div><dt className="font-medium">最终 Prompt</dt><dd className="mt-1 whitespace-pre-wrap">{record.finalPrompt}</dd></div>
                            {record.routingReason && <div><dt className="font-medium">路由</dt><dd>{record.routingReason}</dd></div>}
                            {record.qa.repairPrompt && <div><dt className="font-medium">修复记录</dt><dd>{record.qa.repairPrompt}</dd></div>}
                          </dl>
                        </details>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
          {msg.tool_calls && msg.tool_calls.length > 0 && (
            <div className="precision-tool-call-card mt-3 overflow-hidden">
              <button
                onClick={onToggleExpand}
                className="precision-disclosure-button w-full flex items-center gap-1.5 px-3 py-1.5 text-xs"
              >
                <Wrench size={12} />
                <span>使用了 {msg.tool_calls.length} 个工具</span>
                <ChevronDown
                  size={12}
                  className={`ml-auto transition-transform ${expanded ? 'rotate-180' : ''}`}
                />
              </button>
              {expanded && (
                <div className="px-3 pb-2 space-y-1.5">
                  {msg.tool_calls.map((tc: NonNullable<ChatMessage['tool_calls']>[number], ti: number) => (
                    <div key={ti} className="precision-tool-call-entry text-[11px] font-mono px-2.5 py-1.5">
                      <span className="precision-code-accent font-medium">{tc.function.name}</span>
                      <pre className="precision-code-copy mt-0.5 whitespace-pre-wrap break-all">
                        {(() => {
                          try {
                            return JSON.stringify(JSON.parse(tc.function.arguments), null, 2);
                          } catch {
                            return tc.function.arguments;
                          }
                        })()}
                      </pre>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>

        {/* Action bar — subdued until the message is hovered or focused. */}
        <div className={`precision-message-actions flex items-center ${actionGap} flex-wrap`}>
          {versions.length > 0 && onSwitchVersion && (
            <div className="flex items-center gap-0.5 mr-1">
              <button
                onClick={() => {
                  if (activeV === -1) onSwitchVersion(versions.length - 1);
                  else if (activeV > 0) onSwitchVersion(activeV - 1);
                }}
                disabled={activeV === 0}
                className="precision-inline-action p-1 disabled:opacity-30"
                title="上一个版本"
              >
                <ChevronLeft size={versionIconSize} />
              </button>
              <span className="precision-message-meta text-[10px] tabular-nums">
                {activeV === -1 ? versionCount : activeV + 1}/{versionCount}
              </span>
              <button
                onClick={() => {
                  if (activeV === versions.length - 1) onSwitchVersion(-1);
                  else if (activeV >= 0) onSwitchVersion(activeV + 1);
                }}
                disabled={activeV === -1}
                className="precision-inline-action p-1 disabled:opacity-30"
                title="下一个版本"
              >
                <ChevronRight size={versionIconSize} />
              </button>
            </div>
          )}
          {isLastAssistant && !loading && onRegenerate && (
            <button
              onClick={onRegenerate}
              className={`precision-message-action flex items-center gap-1 ${actionPadding} ${actionTextSize}`}
              title="重新生成"
            >
              <RefreshCw size={actionIconSize} />
              <span>重新生成</span>
            </button>
          )}
          <button
            onClick={() => onCopy(msgContent)}
            className={`precision-message-action flex items-center gap-1 ${actionPadding} ${actionTextSize}`}
            title="复制消息"
          >
            {copied ? <Check size={actionIconSize} /> : <Copy size={actionIconSize} />}
            <span>{copied ? '已复制' : '复制'}</span>
          </button>
          {msgStats && (
            <span
              className="precision-message-meta text-[10px] tabular-nums ml-auto"
              title={`TTFT ${msgStats.ttftMs}ms · 总 ${msgStats.totalMs}ms · 输入 ${msgStats.inputTokens}t · 输出 ${msgStats.outputTokens}t`}
            >
              {msgStats.totalMs}ms · {msgStats.outputTokens}t
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * Collapsible block showing the model's extended-thinking content.
 * Auto-expands while streaming, auto-collapses once the answer arrives.
 */
function ThinkingBlock({ thinking, streaming }: { thinking: string; streaming: boolean }) {
  const [override, setOverride] = useState<boolean | null>(null);
  const expanded = override !== null ? override : streaming;
  return (
    <div className="precision-thinking-block mb-2 overflow-hidden">
      <button
        onClick={() => setOverride(!expanded)}
        className="precision-thinking-header w-full flex items-center gap-1.5 px-3 py-1.5 text-[11px]"
      >
        <Brain size={12} className={streaming ? "animate-pulse" : ""} />
        <span className="font-medium">{streaming ? "思考中…" : "思考过程"}</span>
        <span className="precision-thinking-meta tabular-nums">
          {thinking.length} 字
        </span>
        <ChevronDown
          size={12}
          className={`ml-auto transition-transform ${expanded ? "rotate-180" : ""}`}
        />
      </button>
      {expanded && (
        <div className="precision-thinking-copy px-3 pb-2 text-[12px] leading-relaxed whitespace-pre-wrap font-mono max-h-[300px] overflow-y-auto">
          {thinking}
        </div>
      )}
    </div>
  );
}

const MessageBubble = memo(MessageBubbleInner, (prev, next) => {
  if (prev.msg !== next.msg) return false;
  if (prev.isLastAssistant !== next.isLastAssistant) return false;
  if (prev.loading !== next.loading) return false;
  if (prev.expanded !== next.expanded) return false;
  if (prev.copied !== next.copied) return false;
  if (prev.compact !== next.compact) return false;
  return true;
});

export default MessageBubble;
