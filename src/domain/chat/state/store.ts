import { useState, useCallback, useRef, useEffect } from 'react';
import { getModels, type ThinkingLevel } from '@/core/config';
import { load, save } from '@/core/storage';
import { streamLlm } from '@/core/llm';
import { generateId } from '@/core/utils/id';
import { useAgentChat, createAgentSession, controlAgentHarness, updateAgentRuntimeThinking } from './agentStore';
import { startStream, endStream, abortStream, setStreamError, useStreamState } from './streamRegistry';
import { uploadImages } from './imageUpload';
import { buildAgentStreamCallbacks } from './agentCallbacks';
import { reduceTimeline, backgroundOpenTools } from './timelineReducer';
import { AgentSessionCoordinator } from './agentSessionCoordinator';

export { getModels, getModelInfo } from '@/core/config';

import { apiFetch } from '@/core/utils';
import type { AiModel, MessageStats, ChatMessageVersion, ChatMessage, ChatSession, ChatFolder, ChatAttachment, ContextUsageSnapshot } from '../types';

export type { AiModel, MessageStats, ChatMessageVersion, ChatMessage, ChatSession, ChatFolder };

// Helper: get message content respecting activeVersion
export function getMessageContent(msg: ChatMessage): string {
  if (msg.activeVersion !== undefined && msg.activeVersion >= 0 && msg.versions && msg.versions[msg.activeVersion]) {
    return msg.versions[msg.activeVersion].content;
  }
  return msg.content;
}

export function getMessageTimeline(msg: ChatMessage) {
  if (msg.activeVersion !== undefined && msg.activeVersion >= 0 && msg.versions?.[msg.activeVersion]) {
    return msg.versions[msg.activeVersion].timeline;
  }
  return msg.timeline;
}

export function getMessageDecisionTrace(msg: ChatMessage) {
  if (msg.activeVersion !== undefined && msg.activeVersion >= 0 && msg.versions?.[msg.activeVersion]) {
    return msg.versions[msg.activeVersion].decisionTrace;
  }
  return msg.decisionTrace;
}

export function getMessageImages(msg: ChatMessage): string[] | undefined {
  if (msg.activeVersion !== undefined && msg.activeVersion >= 0 && msg.versions && msg.versions[msg.activeVersion]) {
    return msg.versions[msg.activeVersion].generatedImages;
  }
  return msg.generatedImages;
}

export function getMessageImageRecords(msg: ChatMessage) {
  if (msg.activeVersion !== undefined && msg.activeVersion >= 0 && msg.versions?.[msg.activeVersion]) {
    return msg.versions[msg.activeVersion].generatedImageRecords;
  }
  return msg.generatedImageRecords;
}

export function getMessageStats(msg: ChatMessage): MessageStats | undefined {
  if (msg.activeVersion !== undefined && msg.activeVersion >= 0 && msg.versions && msg.versions[msg.activeVersion]) {
    return msg.versions[msg.activeVersion].stats;
  }
  return msg.stats;
}

/** Archive the current answer version and reset all turn-local projection state. */
export function prepareMessageForRegeneration(msg: ChatMessage, timestamp = Date.now()): ChatMessage {
  const savedVersion: ChatMessageVersion = {
    content: msg.content,
    model: msg.model,
    thinkingLevel: msg.thinkingLevel,
    timeline: msg.timeline,
    decisionTrace: msg.decisionTrace,
    generatedImages: msg.generatedImages,
    generatedImageRecords: msg.generatedImageRecords,
    timestamp,
    stats: msg.stats,
    tokenUsage: msg.tokenUsage,
  };
  return {
    ...msg,
    versions: [...(msg.versions || []), savedVersion],
    activeVersion: -1,
    content: '',
    model: undefined,
    thinkingLevel: undefined,
    timeline: undefined,
    thinking: undefined,
    progressText: undefined,
    toolTrace: undefined,
    decisionTrace: undefined,
    decisionPrompt: undefined,
    error: undefined,
    generatedImages: undefined,
    generatedImageRecords: undefined,
    stats: undefined,
    tokenUsage: undefined,
  };
}


export const NOTE_ASSISTANT_SYSTEM_PROMPT = (noteTitle: string, noteContent: string) => `你是「笔记写作助手」，正在帮助用户撰写和修改笔记。

## 当前笔记信息
- 标题：${noteTitle || '未命名'}
- 现有内容：
\`\`\`
${noteContent || '（空）'}
\`\`\`

## 你的能力
- 帮用户续写、扩写、改写、润色笔记内容
- 根据用户的描述生成新的段落或章节
- 将用户的口头描述转化为结构化的笔记
- 提供写作建议和思路启发
- 帮用户总结、提炼要点
- 如果你认为对话中产生的内容很有价值，可以主动问用户「是否需要将这部分内容添加到当前笔记中？」

## 精确编辑能力
当用户要求精确修改笔记的某一部分时（如"在第二段之后插入xxx"、"删除第二段"、"删除包含xxx的段落"、"在末尾追加xxx"、"替换第三段为xxx"），你必须：
1. 分析当前笔记内容的段落结构（按两个换行\\n\\n分割的文本块，标题行也算独立段落）
2. 根据用户指令精确定位并修改对应段落
3. 返回修改后的**完整笔记内容**，不要只输出修改的部分，不要添加解释说明

## 行为准则
- 直接输出可用于笔记的 Markdown 内容，不要加多余的前缀说明
- 保持与现有内容的风格一致
- 如果需要引用现有内容，简要提及即可
- 用户说"插入到笔记"时，输出完整的内容段落，用户会自行复制粘贴
- 保持简洁高效，避免冗长解释`;



export function useAiChat({
  sessionKey,
  initialMessages = [],
  systemPrompt,
  onMessagesChange,
  useAgent = true,
  serverManagedPrompt = false,
  cwd = '',
  initialAgentSessionId = null,
  onAgentSessionCreated,
  onContextUsage,
  requirePermissionApproval = false,
}: {
  sessionKey?: string;
  initialMessages?: ChatMessage[];
  systemPrompt?: string;
  onMessagesChange?: (sessionId: string, msgs: ChatMessage[]) => void;
  useAgent?: boolean;
  /** When true, agent service manages the system prompt (with data catalog + skills).
   *  The frontend systemPrompt is only used for direct LLM fallback. */
  serverManagedPrompt?: boolean;
  /** Session-level working directory (absolute path). */
  cwd?: string;
  /** Restored pwcli session id when switching chat sessions. */
  initialAgentSessionId?: string | null;
  /** Called when the pwcli backend session ID is created/resolved (for background task correlation). */
  onAgentSessionCreated?: (agentSessionId: string, sessionKey: string) => void;
  /** Provider 上报的最近一次活动上下文，交给 ChatSession 持久化。 */
  onContextUsage?: (usage: ContextUsageSnapshot) => void;
  requirePermissionApproval?: boolean;
} = {}) {
  const [messages, setMessages] = useState<ChatMessage[]>(initialMessages);
  const draftKey = `pwb_chat_draft_${sessionKey || '_anon'}`;
  const [input, setInputRaw] = useState(() => {
    return sessionStorage.getItem(draftKey) || '';
  });
  const setInput = useCallback((v: string) => {
    setInputRaw(v);
    if (v) sessionStorage.setItem(draftKey, v);
    else sessionStorage.removeItem(draftKey);
  }, [draftKey]);
  const [model, setModel] = useState<AiModel>(getModels()[0]?.name ?? '');
  const [thinkingLevel, setThinkingLevelState] = useState<ThinkingLevel>(() => {
    const saved = load<string>('thinking_level', 'high');
    return ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra'].includes(saved)
      ? saved as ThinkingLevel
      : 'high';
  });
  const thinkingLevelRef = useRef(thinkingLevel);
  thinkingLevelRef.current = thinkingLevel;
  const [effectiveThinkingLevel, setEffectiveThinkingLevel] = useState<ThinkingLevel>(thinkingLevel);
  const [pendingThinkingLevel, setPendingThinkingLevel] = useState<ThinkingLevel | null>(null);
  const thinking = thinkingLevel !== 'off';
  const setThinkingLevel = useCallback((level: ThinkingLevel) => {
    setThinkingLevelState(level);
    thinkingLevelRef.current = level;
    if (!streamingRef.current) setEffectiveThinkingLevel(level);
    save('thinking_level', level);
  }, []);
  const setThinking = useCallback((enabled: boolean) => {
    setThinkingLevel(enabled ? 'high' : 'off');
  }, [setThinkingLevel]);
  const streamingRef = useRef(false);
  const lastKeyRef = useRef(sessionKey);
  const systemPromptRef = useRef(systemPrompt);
  systemPromptRef.current = systemPrompt;
  const cwdRef = useRef(cwd);
  cwdRef.current = cwd;
  const sessionKeyRef = useRef(sessionKey);
  sessionKeyRef.current = sessionKey;
  const targetKey = sessionKey || '_anon';
  const { loading, error } = useStreamState(targetKey);

  const handleRuntimeUpdate = useCallback((update: { callIndex: number; thinkingLevel: ThinkingLevel }) => {
    setEffectiveThinkingLevel(update.thinkingLevel);
    setPendingThinkingLevel(current => current === update.thinkingLevel ? null : current);
  }, []);

  useEffect(() => {
    if (loading) return;
    setPendingThinkingLevel(null);
    setEffectiveThinkingLevel(thinkingLevelRef.current);
  }, [loading]);

  // Agent service integration
  const agentChat = useAgentChat();
  const [agentSessionId, setAgentSessionId] = useState<string | null>(initialAgentSessionId);
  const agentSessionRef = useRef<string | null>(initialAgentSessionId);
  agentSessionRef.current = agentSessionId;

  const setThinkingLevelForNextCall = useCallback(async (level: ThinkingLevel) => {
    const previous = thinkingLevelRef.current;
    setThinkingLevel(level);
    const activeSessionId = agentSessionRef.current;
    if (!streamingRef.current || !activeSessionId) {
      setPendingThinkingLevel(null);
      setEffectiveThinkingLevel(level);
      return;
    }
    setPendingThinkingLevel(level);
    try {
      await updateAgentRuntimeThinking(activeSessionId, level);
    } catch (error) {
      setPendingThinkingLevel(null);
      setThinkingLevel(previous);
      setEffectiveThinkingLevel(previous);
      throw error;
    }
  }, [setThinkingLevel]);
  const agentSessionsRef = useRef(new Map<string, string>());
  if (initialAgentSessionId && !agentSessionsRef.current.has(targetKey)) {
    agentSessionsRef.current.set(targetKey, initialAgentSessionId);
  }
  const sessionCoordinatorRef = useRef<AgentSessionCoordinator>();
  if (!sessionCoordinatorRef.current) {
    sessionCoordinatorRef.current = new AgentSessionCoordinator(createAgentSession);
  }

  const onAgentSessionCreatedRef = useRef(onAgentSessionCreated);
  onAgentSessionCreatedRef.current = onAgentSessionCreated;
  const onContextUsageRef = useRef(onContextUsage);
  onContextUsageRef.current = onContextUsage;

  const bindAgentSession = useCallback((target: string, id: string) => {
    agentSessionsRef.current.set(target, id);
    if ((sessionKeyRef.current || '_anon') === target) {
      agentSessionRef.current = id;
      setAgentSessionId(id);
    }
    onAgentSessionCreatedRef.current?.(id, target);
  }, []);

  const ensureAgentSession = useCallback(async (target: string, workspace: string) => {
    const existing = agentSessionsRef.current.get(target);
    if (existing) return existing;
    const id = await sessionCoordinatorRef.current!.ensure(target, workspace);
    bindAgentSession(target, id);
    return id;
  }, [bindAgentSession]);

  const ensureCurrentAgentSession = useCallback(() => {
    const target = sessionKeyRef.current || '_anon';
    return ensureAgentSession(target, cwdRef.current);
  }, [ensureAgentSession]);

  // Create agent session when useAgent is enabled.
  useEffect(() => {
    const target = sessionKey || '_anon';
    if (useAgent && cwd && !agentSessionsRef.current.get(target)) {
      void ensureAgentSession(target, cwd).catch(console.error);
    }
  }, [cwd, ensureAgentSession, sessionKey, useAgent]);

  // Sync with external session when key changes (e.g. switching sessions)
  useEffect(() => {
    if (sessionKey !== lastKeyRef.current) {
      lastKeyRef.current = sessionKey;
      setMessages(initialMessages);
      setInputRaw(sessionStorage.getItem(draftKey) || '');
      const restored = initialAgentSessionId ?? agentSessionsRef.current.get(targetKey) ?? null;
      agentSessionRef.current = restored;
      setAgentSessionId(restored);
    }
  }, [sessionKey, initialMessages, initialAgentSessionId, draftKey, targetKey]);

  // Sync externally injected messages (e.g. background task completion) while not streaming
  const bgSyncedRef = useRef(new Set<string>());
  useEffect(() => {
    if (streamingRef.current) return;
    const lastExternal = initialMessages[initialMessages.length - 1];
    if (!lastExternal?.backgroundTaskId || lastExternal.source !== 'background') return;
    const tid = lastExternal.backgroundTaskId;
    if (bgSyncedRef.current.has(tid)) return;
    bgSyncedRef.current.add(tid);
    setMessages(prev => {
      if (prev.some(m => m.backgroundTaskId === tid)) return prev;
      return [...prev, lastExternal];
    });
  }, [initialMessages]);

  // Throttle onMessagesChange during streaming to avoid JSON.stringify-ing
  // the entire sessions array (potentially 3MB) on every token delta.
  const throttleRef = useRef<{
    timer: ReturnType<typeof setTimeout> | null;
    pending: { target: string; msgs: ChatMessage[] } | null;
  }>({ timer: null, pending: null });

  const flushMessagesPersist = useCallback(() => {
    const t = throttleRef.current;
    if (t.timer !== null) {
      clearTimeout(t.timer);
      t.timer = null;
    }
    if (t.pending !== null) {
      onMessagesChange?.(t.pending.target, t.pending.msgs);
      t.pending = null;
    }
  }, [onMessagesChange]);

  const updateMessages = useCallback((target: string, next: ChatMessage[]) => {
    // Always update local React state for smooth streaming UI
    if (target === (sessionKeyRef.current || '_anon')) {
      setMessages(next);
    }
    // Throttle persistence during streaming (save at most once per 500ms)
    if (streamingRef.current) {
      const t = throttleRef.current;
      t.pending = { target, msgs: next };
      if (t.timer === null) {
        t.timer = setTimeout(() => {
          t.timer = null;
          if (t.pending !== null) {
            onMessagesChange?.(t.pending.target, t.pending.msgs);
            t.pending = null;
          }
        }, 500);
      }
    } else {
      onMessagesChange?.(target, next);
    }
  }, [onMessagesChange]);

  const sendMessage = useCallback(
    async (
      content: string,
      images?: string[],
      contextAttachments?: ChatAttachment[],
      generatedImageReferences?: string[],
    ) => {
      if (!content.trim()
        && (!images || images.length === 0)
        && (!contextAttachments || contextAttachments.length === 0)
        && (!generatedImageReferences || generatedImageReferences.length === 0)) return;
      const target = sessionKeyRef.current || '_anon';
      const controller = startStream(target);
      const signal = controller.signal;
      const update = (msgs: ChatMessage[]) => updateMessages(target, msgs);

      // Upload base64 images to backend filesystem to avoid localStorage quota exhaustion
      let resolvedImages = images;
      if (images && images.length > 0) {
        resolvedImages = await uploadImages(images);
      }

      // 按附件类型和会话模式构建上下文前缀
      let contextPrefix = '';
      if (contextAttachments && contextAttachments.length > 0) {
        const isAgent = useAgent;
        const parts: string[] = [];
        for (const a of contextAttachments) {
          if (a.type === 'file' && isAgent && a.path) {
            const sizeHint = a.size != null
              ? ` (${a.size < 1024 ? a.size + 'B' : (a.size / 1024).toFixed(0) + 'KB'})`
              : '';
            parts.push(
              `[引用文件: ${a.path}${sizeHint}]\n` +
              `请在需要时使用 read 工具查看内容（可用 offset/limit 按行号读片段）。对于大文件，可先读开头若干行，或用 bash 执行 wc -l 了解结构和行数。`
            );
          } else if (a.type === 'file' && !isAgent && a.path) {
            try {
              const data = await apiFetch<{ success?: boolean; isBinary?: boolean; content?: string }>(
                `/api/fs/read?path=${encodeURIComponent(a.path)}`
              );
              if (data.success && !data.isBinary && data.content) {
                const text = data.content.length > 8000
                  ? data.content.slice(0, 8000) + '\n...(已截断)'
                  : data.content;
                parts.push(`[参考资料: ${a.title}]\n\n${text}`);
              } else {
                parts.push(`[参考资料: ${a.title}]（二进制文件）路径: ${a.path}`);
              }
            } catch {
              parts.push(`[引用文件: ${a.path}]（读取失败）`);
            }
          } else {
            parts.push(`[参考资料: ${a.title}]\n\n${a.content}`);
          }
        }
        contextPrefix = parts.join('\n\n---\n\n') + '\n\n---\n\n';
      }

      const userMsg: ChatMessage = {
        id: generateId(),
        role: 'user',
        content: contextPrefix + content.trim(),
        ...(resolvedImages && resolvedImages.length > 0 ? { images: resolvedImages } : {}),
        ...(generatedImageReferences && generatedImageReferences.length > 0
          ? { generatedImages: generatedImageReferences }
          : {}),
      };
      let currentMessages = [...messages, userMsg];
      update(currentMessages);
      setInput('');
      streamingRef.current = true;

      // Agent service path (supports text + images; image generation models fall back to legacy)
      if (useAgent) {
        let sessionId = agentSessionsRef.current.get(target) ?? null;
        if (!sessionId) {
          try {
            sessionId = await ensureAgentSession(target, cwdRef.current);
          } catch (e) {
            console.error('Failed to create agent session:', e);
          }
        }
        if (sessionId) {
          try {
            const assistantMsg: ChatMessage = {
              id: generateId(), role: 'assistant', content: '', model, thinkingLevel,
            };
            currentMessages = [...currentMessages, assistantMsg];
            const assistantIndex = currentMessages.length - 1;
            update(currentMessages);

            // Remove the empty assistant placeholder before sending to agent service,
            // as LLM APIs treat trailing empty assistant messages as conversation end.
            const messagesToSend = currentMessages.filter((m, i, arr) => {
              if (m.systemNotice) return false;
              if (i < arr.length - 1) return true;
              return m.role !== 'assistant' || m.content.trim() !== '' || m.tool_calls?.length;
            });

            const callbacks = buildAgentStreamCallbacks({
              getMessages: () => currentMessages,
              setMessages: (msgs) => { currentMessages = msgs; update(currentMessages); },
              assistantIndex,
              target,
              onRuntimeUpdate: handleRuntimeUpdate,
            });
            const { flushPendingProgress, finalizeTimeline, ...streamCallbacks } = callbacks;
            await agentChat.streamMessage(messagesToSend, {
              sessionId,
              sessionName: sessionKeyRef.current || undefined,
              systemPrompt: serverManagedPrompt ? undefined : systemPromptRef.current,
              model,
              thinking,
              thinkingLevel,
              cwd: cwdRef.current || undefined,
              requirePermissionApproval,
              signal,
              ...streamCallbacks,
              onContextUsage: usage => onContextUsageRef.current?.(usage),
              onDone: (usage) => {
                flushPendingProgress();
                finalizeTimeline();
                // Mark any still-running tool traces as backgrounded (auto-promoted by pwcli)
                const msgs = currentMessages;
                const last = msgs[assistantIndex];
                if (last && (usage || last.toolTrace?.some(t => t.status === 'running'))) {
                  const updated = [...msgs];
                  updated[assistantIndex] = {
                    ...last,
                    tokenUsage: usage ?? last.tokenUsage,
                    toolTrace: last.toolTrace?.map(t =>
                      t.status === 'running' ? { ...t, status: 'backgrounded' as const } : t
                    ),
                  };
                  currentMessages = updated;
                  update(currentMessages);
                }
              },
            });
          } catch (err) {
            if (err instanceof Error && err.name !== 'AbortError') {
              const errMsg = err.message || 'Agent service error';
              setStreamError(target, errMsg);
              const lastIdx = currentMessages.length - 1;
              const last = currentMessages[lastIdx];
              if (last?.role === 'assistant' && !last.content) {
                const updated = [...currentMessages];
                updated[lastIdx] = { ...last, error: errMsg };
                currentMessages = updated;
                update(currentMessages);
              }
            }
          } finally {
            flushMessagesPersist();
            streamingRef.current = false;
            endStream(target);
          }
          return;
        }
        // pwcli session creation failed — report error instead of silent fallback
        const errMsg = 'Daemon 未启动，请运行 pwcli daemon start';
        setStreamError(target, errMsg);
        const assistantMsg: ChatMessage = { id: generateId(), role: 'assistant', content: '', error: errMsg };
        currentMessages = [...currentMessages, assistantMsg];
        update(currentMessages);
        flushMessagesPersist();
        streamingRef.current = false;
        endStream(target);
        return;
      }

      // Direct mode: simple streaming without tools (no agent loop)
      try {
        const assistantMsg: ChatMessage = {
          id: generateId(), role: 'assistant', content: '', model, thinkingLevel,
        };
        currentMessages = [...currentMessages, assistantMsg];
        const assistantIndex = currentMessages.length - 1;
        update(currentMessages);

        const messagesToSend = currentMessages
          .filter((m, i, arr) => !m.systemNotice && (i < arr.length - 1 || m.role !== 'assistant' || m.content.trim() !== ''))
          .map(m => ({ role: m.role, content: m.content, images: m.images, tool_calls: m.tool_calls, tool_call_id: m.tool_call_id }));

        let prevThinking = '';
        let prevContent = '';
        for await (const event of streamLlm({
          model,
          messages: messagesToSend,
          systemPrompt: systemPromptRef.current,
          temperature: 0.7,
          stream: true,
          thinking,
          thinkingLevel,
        }, signal)) {
          if (event.type === 'delta') {
            // streamLlm 的 content/thinking 为累积值，diff 成增量后走同一 reducer
            let msg = currentMessages[assistantIndex];
            const thinkingNow = event.thinking ?? '';
            const contentNow = event.content ?? '';
            if (thinkingNow.length > prevThinking.length) {
              msg = reduceTimeline(msg, { type: 'thinking_delta', delta: thinkingNow.slice(prevThinking.length) });
              prevThinking = thinkingNow;
            }
            if (contentNow.length > prevContent.length) {
              msg = reduceTimeline(msg, { type: 'text_delta', delta: contentNow.slice(prevContent.length) });
              prevContent = contentNow;
            }
            const updated = [...currentMessages];
            updated[assistantIndex] = { ...msg, generatedImages: event.generatedImages };
            currentMessages = updated;
            update(currentMessages);
          } else if (event.type === 'error') {
            throw new Error(event.message);
          }
        }
      } catch (err) {
        if (err instanceof Error && err.name !== 'AbortError') {
          setStreamError(target, err.message || '网络错误，请稍后重试');
          const lastMsg = currentMessages[currentMessages.length - 1];
          if (lastMsg?.role === 'assistant') {
            const closedMsg = reduceTimeline(lastMsg, { type: 'error' });
            if (closedMsg.content === '' && !closedMsg.tool_calls?.length) {
              currentMessages = currentMessages.slice(0, -1);
            } else {
              currentMessages = [...currentMessages.slice(0, -1), closedMsg];
            }
            update(currentMessages);
          }
        }
      } finally {
        const lastMsg = currentMessages[currentMessages.length - 1];
        if (lastMsg?.role === 'assistant' && lastMsg.timeline?.length) {
          // 正常结束/中断时关闭开放项（error 路径已关闭，done 为幂等）
          currentMessages = [...currentMessages.slice(0, -1), reduceTimeline(lastMsg, { type: 'done' })];
          update(currentMessages);
        }
        flushMessagesPersist();
        streamingRef.current = false;
        endStream(target);
      }
    },
    [messages, model, updateMessages, flushMessagesPersist, useAgent, agentChat, thinking, thinkingLevel, serverManagedPrompt, setInput, requirePermissionApproval, ensureAgentSession, handleRuntimeUpdate]
  );

  const retry = useCallback(async () => {
    if (messages.length === 0) return;
    const lastMsg = messages[messages.length - 1];
    // Allow retry if last assistant is empty (failed streaming placeholder) or has tool calls
    if (lastMsg.role === 'assistant' && lastMsg.content !== '' && !lastMsg.tool_calls?.length) return;

    const target = sessionKeyRef.current || '_anon';
    const controller = startStream(target);
    const signal = controller.signal;
    const update = (msgs: ChatMessage[]) => updateMessages(target, msgs);
    streamingRef.current = true;

    // Remove the last incomplete assistant message if present
    let currentMessages = [...messages];
    if (lastMsg.role === 'assistant') {
      currentMessages = currentMessages.slice(0, -1);
    }

    // Retry via pwcli agent (or direct streaming if no agent)
    if (useAgent && agentSessionId) {
      try {
        const assistantMsg: ChatMessage = {
          id: generateId(), role: 'assistant', content: '', model, thinkingLevel,
        };
        currentMessages = [...currentMessages, assistantMsg];
        const assistantIndex = currentMessages.length - 1;
        update(currentMessages);

        const messagesToSend = currentMessages.filter((m, i, arr) => {
          if (m.systemNotice) return false;
          if (i < arr.length - 1) return true;
          return m.role !== 'assistant' || m.content.trim() !== '' || m.tool_calls?.length;
        });

        const callbacks = buildAgentStreamCallbacks({
          getMessages: () => currentMessages,
          setMessages: (msgs) => { currentMessages = msgs; update(currentMessages); },
          assistantIndex,
          target,
          onRuntimeUpdate: handleRuntimeUpdate,
        });
        const { flushPendingProgress, finalizeTimeline, ...streamCallbacks } = callbacks;

        await agentChat.streamMessage(messagesToSend, {
          sessionId: agentSessionId,
          sessionName: sessionKeyRef.current || undefined,
          systemPrompt: serverManagedPrompt ? undefined : systemPromptRef.current,
          model,
          thinking,
          thinkingLevel,
          cwd: cwdRef.current || undefined,
          requirePermissionApproval,
          signal,
          ...streamCallbacks,
          onContextUsage: usage => onContextUsageRef.current?.(usage),
          onDone: (usage) => {
            flushPendingProgress();
            finalizeTimeline();
            if (!usage) return;
            const last = currentMessages[assistantIndex];
            if (!last) return;
            const updated = [...currentMessages];
            updated[assistantIndex] = { ...last, tokenUsage: usage };
            currentMessages = updated;
            update(currentMessages);
          },
        });
      } catch (err) {
        if (err instanceof Error && err.name !== 'AbortError') {
          setStreamError(target, err.message || 'Agent service error');
        }
      } finally {
        flushMessagesPersist();
        streamingRef.current = false;
        endStream(target);
      }
      return;
    }

    // Direct mode streaming (no tools)
    try {
      const assistantMsg: ChatMessage = {
        id: generateId(), role: 'assistant', content: '', model, thinkingLevel,
      };
      currentMessages = [...currentMessages, assistantMsg];
      const assistantIndex = currentMessages.length - 1;
      update(currentMessages);

      const messagesToSend = currentMessages
          .filter((m, i, arr) => !m.systemNotice && (i < arr.length - 1 || m.role !== 'assistant' || m.content.trim() !== ''))
          .map(m => ({ role: m.role, content: m.content, images: m.images, tool_calls: m.tool_calls, tool_call_id: m.tool_call_id }));

      for await (const event of streamLlm({
        model,
        messages: messagesToSend,
        systemPrompt: systemPromptRef.current,
        temperature: 0.7,
        stream: true,
        thinking,
        thinkingLevel,
      }, signal)) {
        if (event.type === 'delta') {
          const updated = [...currentMessages];
          updated[assistantIndex] = {
            ...updated[assistantIndex],
            content: event.content,
            thinking: event.thinking,
            generatedImages: event.generatedImages,
          };
          currentMessages = updated;
          update(currentMessages);
        } else if (event.type === 'error') {
          throw new Error(event.message);
        }
      }
    } catch (err) {
      if (err instanceof Error && err.name !== 'AbortError') {
        setStreamError(target, err.message || '网络错误，请稍后重试');
        const lastMsg = currentMessages[currentMessages.length - 1];
        if (lastMsg?.role === 'assistant' && lastMsg.content === '' && !lastMsg.tool_calls?.length) {
          currentMessages = currentMessages.slice(0, -1);
          update(currentMessages);
        }
      }
    } finally {
      flushMessagesPersist();
      streamingRef.current = false;
      endStream(target);
    }
  }, [messages, model, updateMessages, flushMessagesPersist, useAgent, agentSessionId, agentChat, thinking, thinkingLevel, serverManagedPrompt, requirePermissionApproval, handleRuntimeUpdate]);

  const clearChat = useCallback(async () => {
    const target = sessionKeyRef.current || '_anon';
    const existing = agentSessionsRef.current.get(target);
    if (useAgent) {
      if (!cwdRef.current) return;
      const replacementId = await createAgentSession(target, cwdRef.current);
      try {
        if (existing) {
          await apiFetch(`/api/agent/sessions/${encodeURIComponent(existing)}`, { method: 'DELETE' });
        }
      } catch (reason) {
        void apiFetch(`/api/agent/sessions/${encodeURIComponent(replacementId)}`, { method: 'DELETE' })
          .catch(() => undefined);
        throw reason;
      }
      bindAgentSession(target, replacementId);
    }
    abortStream(target);
    updateMessages(target, []);
    setStreamError(target, null);
  }, [bindAgentSession, updateMessages, useAgent]);

  const stop = useCallback(async () => {
    const target = sessionKeyRef.current || '_anon';
    abortStream(target);
    if (useAgent && agentSessionId) {
      try {
        await controlAgentHarness(agentSessionId, 'abort');
      } catch (reason) {
        const message = reason instanceof Error ? reason.message : 'daemon 未能停止当前任务';
        setStreamError(target, message);
        throw reason;
      }
    }
  }, [useAgent, agentSessionId]);

  const replaceMessages = useCallback((next: ChatMessage[]) => {
    updateMessages(sessionKeyRef.current || '_anon', next);
  }, [updateMessages]);

  // Regenerate the last assistant message, saving the current version to history
  const regenerate = useCallback(async () => {
    if (messages.length === 0) return;
    // Find the last assistant message with content
    let lastAssistantIndex = -1;
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].role === 'assistant' && messages[i].content) {
        lastAssistantIndex = i;
        break;
      }
    }
    if (lastAssistantIndex === -1) return;

    const msg = messages[lastAssistantIndex];
    // Truncate messages after this assistant message (they depend on it)
    let currentMessages = messages.slice(0, lastAssistantIndex + 1);
    currentMessages[lastAssistantIndex] = prepareMessageForRegeneration(msg);
    const target = sessionKeyRef.current || '_anon';
    const controller = startStream(target);
    const signal = controller.signal;
    const update = (msgs: ChatMessage[]) => updateMessages(target, msgs);
    update(currentMessages);
    streamingRef.current = true;

    // Agent service path for regeneration
    if (useAgent && agentSessionId) {
      try {
        const assistantIndex = currentMessages.length - 1;
        const messagesToSend = currentMessages.filter((m, i, arr) => {
          if (m.systemNotice) return false;
          if (i < arr.length - 1) return true;
          return m.role !== 'assistant' || m.content.trim() !== '' || m.tool_calls?.length;
        });

        const callbacks = buildAgentStreamCallbacks({
          getMessages: () => currentMessages,
          setMessages: (msgs) => { currentMessages = msgs; update(currentMessages); },
          assistantIndex,
          target,
          onRuntimeUpdate: handleRuntimeUpdate,
        });
        const { flushPendingProgress, finalizeTimeline, ...streamCallbacks } = callbacks;

        await agentChat.streamMessage(messagesToSend, {
          sessionId: agentSessionId,
          sessionName: sessionKeyRef.current || undefined,
          systemPrompt: serverManagedPrompt ? undefined : systemPromptRef.current,
          model,
          thinking,
          thinkingLevel,
          cwd: cwdRef.current || undefined,
          requirePermissionApproval,
          signal,
          ...streamCallbacks,
          onContextUsage: usage => onContextUsageRef.current?.(usage),
          onDone: (usage) => {
            flushPendingProgress();
            finalizeTimeline();
            const msgs = currentMessages;
            const last = msgs[assistantIndex];
            if (!last) return;
            // 仍 running 的工具转后台，随后关闭全部开放项
            const closed = reduceTimeline(backgroundOpenTools(last), { type: 'done' });
            const updated = [...msgs];
            updated[assistantIndex] = { ...closed, tokenUsage: usage ?? closed.tokenUsage };
            currentMessages = updated;
            update(currentMessages);
          },
        });
      } catch (err) {
        if (err instanceof Error && err.name !== 'AbortError') {
          setStreamError(target, err.message || 'Agent service error');
        }
      } finally {
        flushMessagesPersist();
        streamingRef.current = false;
        endStream(target);
      }
      return;
    }

    // Direct mode streaming fallback for regenerate
    try {
      const assistantIndex = currentMessages.length - 1;
      const messagesToSend = currentMessages
          .filter((m, i, arr) => !m.systemNotice && (i < arr.length - 1 || m.role !== 'assistant' || m.content.trim() !== ''))
          .map(m => ({ role: m.role, content: m.content, images: m.images, tool_calls: m.tool_calls, tool_call_id: m.tool_call_id }));

      let prevThinking = '';
      let prevContent = '';
      for await (const event of streamLlm({
        model,
        messages: messagesToSend,
        systemPrompt: systemPromptRef.current,
        temperature: 0.7,
        stream: true,
        thinking,
        thinkingLevel,
      }, signal)) {
        if (event.type === 'delta') {
          let msg = currentMessages[assistantIndex];
          const thinkingNow = event.thinking ?? '';
          const contentNow = event.content ?? '';
          if (thinkingNow.length > prevThinking.length) {
            msg = reduceTimeline(msg, { type: 'thinking_delta', delta: thinkingNow.slice(prevThinking.length) });
            prevThinking = thinkingNow;
          }
          if (contentNow.length > prevContent.length) {
            msg = reduceTimeline(msg, { type: 'text_delta', delta: contentNow.slice(prevContent.length) });
            prevContent = contentNow;
          }
          const updated = [...currentMessages];
          updated[assistantIndex] = { ...msg, generatedImages: event.generatedImages };
          currentMessages = updated;
          update(currentMessages);
        } else if (event.type === 'error') {
          throw new Error(event.message);
        }
      }
    } catch (err) {
      if (err instanceof Error && err.name !== 'AbortError') {
        setStreamError(target, err.message || '网络错误，请稍后重试');
        const lastMsg = currentMessages[currentMessages.length - 1];
        if (lastMsg?.role === 'assistant') {
          const closedMsg = reduceTimeline(lastMsg, { type: 'error' });
          if (closedMsg.content === '' && !closedMsg.tool_calls?.length) {
            currentMessages = currentMessages.slice(0, -1);
          } else {
            currentMessages = [...currentMessages.slice(0, -1), closedMsg];
          }
          update(currentMessages);
        }
      }
    } finally {
      const lastMsg = currentMessages[currentMessages.length - 1];
      if (lastMsg?.role === 'assistant' && lastMsg.timeline?.length) {
        currentMessages = [...currentMessages.slice(0, -1), reduceTimeline(lastMsg, { type: 'done' })];
        update(currentMessages);
      }
      flushMessagesPersist();
      streamingRef.current = false;
      endStream(target);
    }
  }, [messages, model, updateMessages, flushMessagesPersist, useAgent, agentSessionId, agentChat, thinking, thinkingLevel, serverManagedPrompt, requirePermissionApproval, handleRuntimeUpdate]);

  // Switch between versions of a message
  const switchVersion = useCallback((msgIndex: number, versionIndex: number) => {
    const msg = messages[msgIndex];
    if (!msg || !msg.versions || versionIndex < -1 || versionIndex >= msg.versions.length) return;
    const updated = [...messages];
    updated[msgIndex] = { ...msg, activeVersion: versionIndex };
    updateMessages(sessionKeyRef.current || '_anon', updated);
  }, [messages, updateMessages]);

  const clearError = useCallback(() => {
    setStreamError(targetKey, null);
  }, [targetKey]);

  return {
    messages,
    input,
    setInput,
    loading,
    error,
    clearError,
    model,
    setModel,
    thinking,
    thinkingLevel,
    effectiveThinkingLevel,
    pendingThinkingLevel,
    setThinking,
    setThinkingLevel,
    setThinkingLevelForNextCall,
    sendMessage,
    retry,
    regenerate,
    switchVersion,
    clearChat,
    ensureAgentSession: ensureCurrentAgentSession,
    replaceMessages,
    stop,
  };
}
