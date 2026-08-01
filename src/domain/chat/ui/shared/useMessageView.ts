import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ChatMessage } from '../../types';
import { showToast } from '@/shell';

export interface MessageEntry {
  msg: ChatMessage;
  idx: number;
}

interface UseMessageViewOptions {
  messages: ChatMessage[];
  loading: boolean;
}

const INITIAL_RENDER_COUNT = 40;
const LOAD_MORE_STEP = 30;

/**
 * Shared message-view state used by ChatTab:
 * - visible message filter (system/tool/empty assistant hidden)
 * - last-assistant index (for "regenerate" button placement)
 * - "near bottom" auto-scroll (preserves user's scroll-up)
 * - copy button state with timer cleanup
 * - tool-call expansion state keyed by message id (or fallback)
 */
export function useMessageView({ messages, loading }: UseMessageViewOptions) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stickToBottomRef = useRef(true);

  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
    stickToBottomRef.current = distance < 64;
  }, []);

  useEffect(() => {
    if (stickToBottomRef.current && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages, loading]);

  const allEntries = useMemo<MessageEntry[]>(() => {
    return messages
      .map((m, idx) => ({ msg: m, idx }))
      .filter(({ msg: m }) =>
        m.role !== 'system' &&
        m.role !== 'tool' &&
        !(m.role === 'assistant' && !m.content && m.tool_calls)
      );
  }, [messages]);

  // 窗口化：只渲染最后 N 条，滚到顶可加载更多
  const [renderCount, setRenderCount] = useState(INITIAL_RENDER_COUNT);

  // 切换会话时重置窗口
  const msgCountRef = useRef(messages.length);
  useEffect(() => {
    if (messages.length !== msgCountRef.current) {
      // 新消息到来时不重置（保留已加载的历史）
      if (messages.length < msgCountRef.current) {
        setRenderCount(INITIAL_RENDER_COUNT);
      }
      msgCountRef.current = messages.length;
    }
  }, [messages.length]);

  const hasMore = allEntries.length > renderCount;
  const visibleMessageEntries = useMemo<MessageEntry[]>(() => {
    if (allEntries.length <= renderCount) return allEntries;
    return allEntries.slice(allEntries.length - renderCount);
  }, [allEntries, renderCount]);

  const loadMore = useCallback(() => {
    setRenderCount(prev => Math.min(prev + LOAD_MORE_STEP, allEntries.length));
  }, [allEntries.length]);

  const showAll = useCallback(() => {
    setRenderCount(allEntries.length);
  }, [allEntries.length]);

  const lastAssistantOriginalIdx = useMemo(() => {
    for (let i = visibleMessageEntries.length - 1; i >= 0; i--) {
      if (visibleMessageEntries[i].msg.role === 'assistant') {
        return visibleMessageEntries[i].idx;
      }
    }
    return -1;
  }, [visibleMessageEntries]);

  const messageKey = useCallback(
    (msg: ChatMessage, idx: number) => msg.id ?? `idx-${idx}`,
    []
  );

  // Copy
  const [copiedMsgKey, setCopiedMsgKey] = useState<string | null>(null);
  const copyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const copyMessage = useCallback(async (text: string, key: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedMsgKey(key);
      if (copyTimerRef.current) clearTimeout(copyTimerRef.current);
      copyTimerRef.current = setTimeout(() => setCopiedMsgKey(null), 2000);
    } catch {
      showToast({ message: '复制失败：浏览器拒绝了剪贴板权限', type: 'error' });
    }
  }, []);

  useEffect(() => () => {
    if (copyTimerRef.current) clearTimeout(copyTimerRef.current);
  }, []);

  // Tool call expansion
  const [expandedToolCalls, setExpandedToolCalls] = useState<Set<string>>(new Set());
  const toggleToolExpand = useCallback((key: string) => {
    setExpandedToolCalls(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  return {
    scrollRef,
    handleScroll,
    visibleMessageEntries,
    lastAssistantOriginalIdx,
    messageKey,
    copiedMsgKey,
    copyMessage,
    expandedToolCalls,
    toggleToolExpand,
    hasMore,
    loadMore,
    showAll,
  };
}
