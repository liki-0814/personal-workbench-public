import type { ChatMessage, DecisionOption, DecisionPromptRecord, DecisionTrace, GeneratedImageRecord, ToolTrace } from '../types';
import type { DocumentRef } from '@/domain/documents';
import { setStreamError } from './streamRegistry';
import { uploadImages, isBase64DataUrl } from './imageUpload';

const TOOL_TRACE_ARG_LIMIT = 500;
const TOOL_TRACE_RESULT_LIMIT = 1000;
/** Maximum progress lines kept per trace chip. Excess drops oldest (keeps tail). */
const TOOL_TRACE_PROGRESS_LIMIT = 200;

function truncTraceField(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max)}…[共 ${s.length} 字符]` : s;
}

export interface AgentStreamCallbackParams {
  /** Getter for current messages array (captured via closure in caller) */
  getMessages: () => ChatMessage[];
  /** Setter that updates both the mutable ref and triggers UI update */
  setMessages: (msgs: ChatMessage[]) => void;
  /** Index of the assistant message being streamed into */
  assistantIndex: number;
  /** Stream target key for error reporting */
  target: string;
}

/**
 * Build the agent stream callbacks object used by agentChat.streamMessage().
 * Eliminates duplication between sendMessage and regenerate.
 */
export function buildAgentStreamCallbacks(params: AgentStreamCallbackParams) {
  const { getMessages, setMessages, assistantIndex, target } = params;

  // Throttle progress lines: accumulate per-tool and flush every 150ms
  const pendingProgress = new Map<string, string[]>();
  let progressFlushTimer: ReturnType<typeof setTimeout> | null = null;

  function flushProgress() {
    progressFlushTimer = null;
    if (pendingProgress.size === 0) return;
    const updated = [...getMessages()];
    const cur = updated[assistantIndex];
    if (!cur.toolTrace) { pendingProgress.clear(); return; }
    updated[assistantIndex] = {
      ...cur,
      toolTrace: cur.toolTrace.map(t => {
        const lines = pendingProgress.get(t.id);
        if (!lines) return t;
        const log = t.progressLog ?? [];
        const combined = [...log, ...lines];
        const next = combined.length > TOOL_TRACE_PROGRESS_LIMIT
          ? combined.slice(combined.length - TOOL_TRACE_PROGRESS_LIMIT)
          : combined;
        return { ...t, progressLog: next };
      }),
    };
    pendingProgress.clear();
    setMessages(updated);
  }

  return {
    onDelta: (delta: string) => {
      const updated = [...getMessages()];
      updated[assistantIndex] = { ...updated[assistantIndex], content: updated[assistantIndex].content + delta };
      setMessages(updated);
    },
    onThinkingDelta: (delta: string) => {
      const updated = [...getMessages()];
      updated[assistantIndex] = {
        ...updated[assistantIndex],
        thinking: (updated[assistantIndex].thinking || '') + delta,
      };
      setMessages(updated);
    },
    onStreamReset: (_reason: string) => {
      const updated = [...getMessages()];
      const current = updated[assistantIndex];
      updated[assistantIndex] = {
        ...current,
        content: '',
        thinking: '',
        toolTrace: undefined,
      };
      setMessages(updated);
    },
    onToolCall: ({ id, name }: { id: string; name: string }) => {
      const updated = [...getMessages()];
      const cur = updated[assistantIndex];
      const existing = cur.toolTrace ?? [];
      if (existing.some(t => t.id === id)) return;
      const trace: ToolTrace = { id, name, args: '', status: 'running' };
      updated[assistantIndex] = { ...cur, toolTrace: [...existing, trace] };
      setMessages(updated);
    },
    onToolCallArgsDelta: (id: string, argsDelta: string) => {
      const updated = [...getMessages()];
      const cur = updated[assistantIndex];
      const trace = cur.toolTrace?.find(t => t.id === id);
      if (!trace) return;
      const newArgs = truncTraceField(trace.args + argsDelta, TOOL_TRACE_ARG_LIMIT);
      updated[assistantIndex] = {
        ...cur,
        toolTrace: cur.toolTrace!.map(t => t.id === id ? { ...t, args: newArgs } : t),
      };
      setMessages(updated);
    },
    onToolResult: ({ toolCallId, output, isError, failure }: { toolCallId: string; output: string; isError: boolean; failure?: import('../types').FailureEnvelope }) => {
      // Flush any pending progress for this tool before recording the result
      if (pendingProgress.has(toolCallId)) {
        if (progressFlushTimer) { clearTimeout(progressFlushTimer); progressFlushTimer = null; }
        flushProgress();
      }
      const updated = [...getMessages()];
      const cur = updated[assistantIndex];
      if (!cur.toolTrace) return;
      updated[assistantIndex] = {
        ...cur,
        toolTrace: cur.toolTrace.map(t => t.id === toolCallId ? {
          ...t,
          result: truncTraceField(output, TOOL_TRACE_RESULT_LIMIT),
          isError,
          failure,
          recoveryPhase: undefined,
          status: isError ? 'error' : 'done',
        } : t),
      };
      setMessages(updated);
    },
    onToolRecovery: ({ toolCallId, failure, phase }: { toolCallId: string; failure: import('../types').FailureEnvelope; phase: string }) => {
      const updated = [...getMessages()];
      const cur = updated[assistantIndex];
      if (!cur.toolTrace) return;
      updated[assistantIndex] = {
        ...cur,
        toolTrace: cur.toolTrace.map(t => t.id === toolCallId ? {
          ...t,
          failure,
          recoveryPhase: phase,
          status: phase === 'auto_retrying' ? 'recovering' : t.status,
        } : t),
      };
      setMessages(updated);
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
      const updated = [...getMessages()];
      const current = updated[assistantIndex];
      const existing = current.decisionTrace ?? [];
      if (existing.some(trace => trace.id === id)) return;
      const trace: DecisionTrace = { id, trigger, risk, status: 'reviewing', advisors: [] };
      updated[assistantIndex] = { ...current, decisionTrace: [...existing, trace] };
      setMessages(updated);
    },
    onDecisionAdvisor: ({ id, model, status }: { id: string; model: string; status: string }) => {
      const updated = [...getMessages()];
      const current = updated[assistantIndex];
      if (!current.decisionTrace) return;
      updated[assistantIndex] = {
        ...current,
        decisionTrace: current.decisionTrace.map(trace => trace.id !== id ? trace : {
          ...trace,
          advisors: [...trace.advisors.filter(advisor => advisor.model !== model), { model, status }],
        }),
      };
      setMessages(updated);
    },
    onDecisionResolved: ({ id, outcome, confidence, consensus, rationale }: {
      id: string; outcome: string; confidence: number; consensus: number; rationale: string;
    }) => {
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
      setStreamError(target, err);
      const updated = [...getMessages()];
      if (updated[assistantIndex] && !updated[assistantIndex].content) {
        updated[assistantIndex] = { ...updated[assistantIndex], error: err };
        setMessages(updated);
      }
    },
    /** Flush any buffered progress lines immediately (call before onDone). */
    flushPendingProgress: () => {
      if (progressFlushTimer) { clearTimeout(progressFlushTimer); progressFlushTimer = null; }
      flushProgress();
    },
  };
}
