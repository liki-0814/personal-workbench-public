import { useEffect, useRef } from 'react';
import { apiFetch } from '@/core/utils';
import { generateId } from '@/core/utils/id';
import type { ChatMessage, SessionRuntime } from '../types';

interface SessionSnapshot {
  id: string;
  messages: ChatMessage[];
}

function messageKey(message: ChatMessage): string {
  return JSON.stringify({
    role: message.role,
    content: message.content,
  });
}

function commonPrefixLength(left: ChatMessage[], right: ChatMessage[]): number {
  const limit = Math.min(left.length, right.length);
  let index = 0;
  while (index < limit && messageKey(left[index]) === messageKey(right[index])) index += 1;
  return index;
}

/** Collapse daemon-native assistant/tool cycles into the one assistant bubble used by Chat. */
export function projectDurableSessionMessages(messages: ChatMessage[]): ChatMessage[] {
  const projected: ChatMessage[] = [];
  let assistantParts: ChatMessage[] = [];
  const flushAssistant = () => {
    if (assistantParts.length === 0) return;
    const content = assistantParts.map(message => message.content).join('');
    const generatedImages = assistantParts.flatMap(message => message.generatedImages ?? []);
    if (content || generatedImages.length > 0) {
      projected.push({
        role: 'assistant',
        content,
        ...(generatedImages.length > 0 ? { generatedImages: [...new Set(generatedImages)] } : {}),
      });
    }
    assistantParts = [];
  };

  messages.forEach(message => {
    if (message.role === 'user') {
      flushAssistant();
      projected.push(message);
    } else if (message.role === 'assistant') {
      assistantParts.push(message);
    }
  });
  flushAssistant();
  return projected;
}

/**
 * Adds daemon-journal messages without replacing richer browser message objects.
 * Messages authored after the snapshot request are retained behind the durable suffix.
 */
export function mergeDurableSessionMessages(
  baseline: ChatMessage[],
  current: ChatMessage[],
  durable: ChatMessage[],
): ChatMessage[] {
  const durableProjection = projectDurableSessionMessages(durable);
  if (commonPrefixLength(baseline, current) !== baseline.length) return current;
  if (commonPrefixLength(baseline, durableProjection) !== baseline.length) return current;

  const durableSuffix = durableProjection.slice(baseline.length);
  if (durableSuffix.length === 0) return current;

  const currentPrefix = current.slice(0, baseline.length);
  const currentTail = current.slice(baseline.length);
  const sharedTailLength = commonPrefixLength(currentTail, durableSuffix);
  if (sharedTailLength === durableSuffix.length) return current;

  const durableTail = [
    ...currentTail.slice(0, sharedTailLength),
    ...durableSuffix.slice(sharedTailLength).map(message => ({
      ...message,
      id: message.id || generateId(),
    })),
  ];
  const localNewerTail = currentTail.slice(sharedTailLength);
  return [...currentPrefix, ...durableTail, ...localNewerTail];
}

function runtimeSettled(runtime: SessionRuntime): boolean {
  return runtime.phase === 'idle' || runtime.phase === 'paused' || runtime.phase === 'failed';
}

export function shouldSyncSessionJournal(
  previous: SessionRuntime | null,
  current: SessionRuntime,
): boolean {
  if (!runtimeSettled(current)) return false;
  if (!previous || previous.sessionId !== current.sessionId) return true;
  if (current.lastEventSequence <= previous.lastEventSequence) return false;
  if (previous.phase === 'running' || previous.phase === 'stopping') return true;
  if (previous.queue.length > current.queue.length) return true;
  const claimedIds = new Set(previous.queue
    .filter(item => item.status === 'claimed')
    .map(item => item.id));
  return claimedIds.size > 0 && !current.queue.some(item => claimedIds.has(item.id));
}

export function useSessionJournalSync({
  sessionId,
  runtime,
  localLoading,
  messages,
  onMerge,
}: {
  sessionId?: string;
  runtime: SessionRuntime | null;
  localLoading: boolean;
  messages: ChatMessage[];
  onMerge: (messages: ChatMessage[]) => void;
}) {
  const activeSessionRef = useRef(sessionId);
  const trackedSessionRef = useRef<string>();
  const previousRuntimeRef = useRef<SessionRuntime | null>(null);
  const pendingSequenceRef = useRef<number | null>(null);
  const requestFenceRef = useRef(0);
  const messagesRef = useRef(messages);
  const localLoadingRef = useRef(localLoading);
  const onMergeRef = useRef(onMerge);
  activeSessionRef.current = sessionId;
  messagesRef.current = messages;
  localLoadingRef.current = localLoading;
  onMergeRef.current = onMerge;

  useEffect(() => {
    if (!sessionId) {
      trackedSessionRef.current = undefined;
      previousRuntimeRef.current = null;
      pendingSequenceRef.current = null;
      requestFenceRef.current += 1;
      return;
    }

    if (trackedSessionRef.current !== sessionId) {
      trackedSessionRef.current = sessionId;
      previousRuntimeRef.current = null;
      // A daemon-owned turn may have settled while this session was not open.
      // Treat activation itself as a durable event so the journal projection
      // is refreshed even when no new runtime SSE transition will arrive.
      pendingSequenceRef.current = 0;
      requestFenceRef.current += 1;
    }

    if (runtime?.sessionId === sessionId) {
      const previous = previousRuntimeRef.current;
      previousRuntimeRef.current = runtime;
      if (shouldSyncSessionJournal(previous, runtime)) {
        pendingSequenceRef.current = Math.max(
          pendingSequenceRef.current ?? 0,
          runtime.lastEventSequence,
        );
      }
    }
    if (localLoading || pendingSequenceRef.current === null) return;

    const targetSessionId = sessionId;
    const targetSequence = pendingSequenceRef.current;
    pendingSequenceRef.current = null;
    const baseline = messagesRef.current;
    const requestFence = ++requestFenceRef.current;

    void apiFetch<SessionSnapshot>(
      `/api/agent/sessions/${encodeURIComponent(targetSessionId)}/snapshot`,
    ).then(snapshot => {
      if (
        requestFence !== requestFenceRef.current
        || activeSessionRef.current !== targetSessionId
        || snapshot.id !== targetSessionId
      ) return;
      if (localLoadingRef.current) {
        pendingSequenceRef.current = Math.max(
          pendingSequenceRef.current ?? 0,
          targetSequence,
        );
        return;
      }
      const currentMessages = messagesRef.current;
      const merged = mergeDurableSessionMessages(baseline, currentMessages, snapshot.messages);
      if (merged !== currentMessages) onMergeRef.current(merged);
    }).catch(() => {
      // The next durable runtime event or session activation will retry the snapshot.
    });
  }, [localLoading, runtime, sessionId]);
}
