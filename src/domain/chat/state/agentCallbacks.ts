import type { ChatMessage, DecisionOption, DecisionPromptRecord, DecisionTrace, GeneratedImageRecord } from '../types';
import type { DocumentRef } from '@/domain/documents';
import { setStreamError } from './streamRegistry';
import { uploadImages, isBase64DataUrl } from './imageUpload';
import { reduceTimeline, type TimelineEvent } from './timelineReducer';

export interface AgentStreamCallbackParams {
  /** Getter for current messages array (captured via closure in caller) */
  getMessages: () => ChatMessage[];
  /** Setter that updates both the mutable ref and triggers UI update */
  setMessages: (msgs: ChatMessage[]) => void;
  /** Index of the assistant message being streamed into */
  assistantIndex: number;
  /** Stream target key for error reporting */
  target: string;
  onRuntimeUpdate?: (update: { callIndex: number; thinkingLevel: import('@/core/config').ThinkingLevel }) => void;
}

/**
 * Build the agent stream callbacks object used by agentChat.streamMessage().
 * 过程事件（思考/正文/工具）统一经 reduceTimeline 归约为 msg.timeline
 * （分组/关闭规则见 timelineReducer）；本层只负责进度节流与消息级副作用
 * （图片、文档、决策等独立字段）。
 */
export function buildAgentStreamCallbacks(params: AgentStreamCallbackParams) {
  const { getMessages, setMessages, assistantIndex, target, onRuntimeUpdate } = params;

  // Throttle progress lines: accumulate per-tool and flush every 150ms
  const pendingProgress = new Map<string, string[]>();
  let progressFlushTimer: ReturnType<typeof setTimeout> | null = null;
  let activeRound: number | undefined;

  function commit(next: ChatMessage) {
    const updated = [...getMessages()];
    updated[assistantIndex] = next;
    setMessages(updated);
  }

  function apply(ev: TimelineEvent) {
    commit(reduceTimeline(getMessages()[assistantIndex], ev));
  }

  function flushProgress() {
    progressFlushTimer = null;
    if (pendingProgress.size === 0) return;
    let msg = getMessages()[assistantIndex];
    for (const [id, lines] of pendingProgress) {
      for (const line of lines) msg = reduceTimeline(msg, { type: 'tool_progress', id, line });
    }
    pendingProgress.clear();
    commit(msg);
  }

  return {
    onAssistantSegmentStart: (round: number) => {
      activeRound = round;
      apply({ type: 'segment_start', round });
    },
    onAssistantSegmentEnd: (round: number, hasToolCalls: boolean) => {
      apply({ type: 'segment_end', round, hasToolCalls });
      if (activeRound === round) activeRound = undefined;
    },
    onAssistantSegmentClassified: (round: number, kind: 'narration' | 'candidate' | 'final') => {
      apply({ type: 'segment_classified', round, kind });
    },
    onCandidateDisposition: (round: number, disposition: 'promoted' | 'discarded' | 'superseded') => {
      apply({ type: 'candidate_disposition', round, disposition });
    },
    onRuntimeUpdate: (update: { callIndex: number; thinkingLevel: import('@/core/config').ThinkingLevel }) => {
      const current = getMessages()[assistantIndex];
      if (current) commit({ ...current, thinkingLevel: update.thinkingLevel });
      onRuntimeUpdate?.(update);
    },
    onDelta: (delta: string) => {
      apply({ type: 'text_delta', delta, round: activeRound });
    },
    onThinkingDelta: (delta: string) => {
      apply({ type: 'thinking_delta', delta });
    },
    onStreamReset: (_reason: string) => {
      apply({ type: 'stream_reset' });
    },
    onToolCall: ({ id, name }: { id: string; name: string }) => {
      apply({ type: 'tool_call_start', id, name });
    },
    onToolCallArgsDelta: (id: string, argsDelta: string) => {
      apply({ type: 'tool_call_args_delta', id, delta: argsDelta });
    },
    onToolResult: ({ toolCallId, output, isError, failure }: { toolCallId: string; output: string; isError: boolean; failure?: import('../types').FailureEnvelope }) => {
      // Flush any pending progress for this tool before recording the result
      if (pendingProgress.has(toolCallId)) {
        if (progressFlushTimer) { clearTimeout(progressFlushTimer); progressFlushTimer = null; }
        flushProgress();
      }
      apply({ type: 'tool_result', id: toolCallId, output, isError, failure });
    },
    onToolRecovery: ({ toolCallId, failure, phase }: { toolCallId: string; failure: import('../types').FailureEnvelope; phase: string }) => {
      apply({ type: 'tool_recovery', id: toolCallId, phase, failure });
    },
    onToolProgress: (toolCallId: string, line: string) => {
      const buf = pendingProgress.get(toolCallId) ?? [];
      buf.push(line);
      pendingProgress.set(toolCallId, buf);
      if (!progressFlushTimer) {
        progressFlushTimer = setTimeout(flushProgress, 150);
      }
    },
    onToolImage: async (_toolCallId: string, url: string, _alt: string, record?: GeneratedImageRecord) => {
      // generate_image tool output: append to assistant message's generatedImages[]
      // Upload base64 to backend to avoid localStorage quota exhaustion
      const finalUrl = isBase64DataUrl(url) ? await (async () => { const [uploaded] = await uploadImages([url]); return uploaded; })() : url;
      const updated = [...getMessages()];
      const cur = updated[assistantIndex];
      const existing = cur.generatedImages ?? [];
      if (existing.includes(finalUrl)) return;
      const records = cur.generatedImageRecords ?? [];
      const normalizedRecord = record ? { ...record, url: finalUrl } : undefined;
      updated[assistantIndex] = {
        ...cur,
        generatedImages: [...existing, finalUrl],
        generatedImageRecords: normalizedRecord && !records.some(item => item.id === normalizedRecord.id)
          ? [...records, normalizedRecord]
          : records,
      };
      setMessages(updated);
    },
    onToolDocument: (_toolCallId: string, document: DocumentRef) => {
      const updated = [...getMessages()];
      const current = updated[assistantIndex];
      const existing = current.documentRefs ?? [];
      updated[assistantIndex] = {
        ...current,
        documentRefs: [...existing.filter(item => item.id !== document.id), document],
      };
      setMessages(updated);
    },
    onToolDecision: (_toolCallId: string, decision: DecisionPromptRecord) => {
      const updated = [...getMessages()];
      updated[assistantIndex] = { ...updated[assistantIndex], decisionPrompt: decision };
      setMessages(updated);
    },
    onDecisionStarted: ({ id, trigger, risk }: { id: string; trigger: string; risk: string }) => {
      apply({ type: 'decision_started', id, trigger });
      const updated = [...getMessages()];
      const current = updated[assistantIndex];
      const existing = current.decisionTrace ?? [];
      if (existing.some(trace => trace.id === id)) return;
      const trace: DecisionTrace = { id, trigger, risk, status: 'reviewing', advisors: [] };
      updated[assistantIndex] = { ...current, decisionTrace: [...existing, trace] };
      setMessages(updated);
    },
    onDecisionAdvisor: ({ id, model, status, round, summary }: { id: string; model: string; status: string; round?: number; summary?: string }) => {
      const updated = [...getMessages()];
      const current = updated[assistantIndex];
      if (!current.decisionTrace) return;
      updated[assistantIndex] = {
        ...current,
        decisionTrace: current.decisionTrace.map(trace => trace.id !== id ? trace : {
          ...trace,
          advisors: [
            ...trace.advisors.filter(advisor => !(advisor.model === model && advisor.round === round)),
            { model, status, round, summary },
          ],
        }),
      };
      setMessages(updated);
    },
    onDecisionResolved: ({ id, outcome, confidence, consensus, rationale }: {
      id: string; outcome: string; confidence: number; consensus: number; rationale: string;
    }) => {
      apply({ type: 'decision_resolved', id, outcome });
      const updated = [...getMessages()];
      const current = updated[assistantIndex];
      if (!current.decisionTrace) return;
      updated[assistantIndex] = {
        ...current,
        decisionTrace: current.decisionTrace.map(trace => trace.id === id
          ? { ...trace, status: 'resolved', outcome, confidence, consensus, rationale }
          : trace),
      };
      setMessages(updated);
    },
    onDecisionEscalated: ({ id, rationale, options }: { id: string; rationale: string; options?: DecisionOption[] }) => {
      const updated = [...getMessages()];
      const current = updated[assistantIndex];
      if (!current.decisionTrace) return;
      updated[assistantIndex] = {
        ...current,
        decisionTrace: current.decisionTrace.map(trace => trace.id === id
          ? { ...trace, status: 'escalated', outcome: 'escalate', rationale, options: options ?? [] }
          : trace),
      };
      setMessages(updated);
    },
    onError: (err: string) => {
      if (progressFlushTimer) { clearTimeout(progressFlushTimer); progressFlushTimer = null; }
      flushProgress();
      apply({ type: 'error' });
      setStreamError(target, err);
      const current = getMessages()[assistantIndex];
      if (current && !current.content) {
        commit({ ...current, error: err });
      }
    },
    /** Flush any buffered progress lines immediately (call before onDone). */
    flushPendingProgress: () => {
      if (progressFlushTimer) { clearTimeout(progressFlushTimer); progressFlushTimer = null; }
      flushProgress();
    },
    finalizeTimeline: () => apply({ type: 'done' }),
  };
}
