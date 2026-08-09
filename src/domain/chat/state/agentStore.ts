import { useState, useCallback, useRef, useEffect } from 'react';
import type { ChatMessage, ContextUsageSnapshot, DecisionOption, DecisionPromptRecord, GeneratedImageRecord, TokenUsage } from '../types';
import type { DocumentRef } from '@/domain/documents';
import { requestSyncFromServer } from '@/core/storage';
import { getModelInfo } from '@/core/config';
import type { ThinkingLevel } from '@/core/config';
import { getBackendUrl } from '@/core/config/backendUrl';
import { apiFetch } from '@/core/utils';

export interface AgentSkill {
  name: string;
  description: string;
  has_command: boolean;
}

export interface AgentChatOptions {
  sessionId: string;
  /** 前端 ChatSession.id，传给后端用于后台任务回调匹配 */
  sessionName?: string;
  systemPrompt?: string;
  /** 前端选择的模型 id；设置后透传给 pwcli 用 per-request provider 覆盖 */
  model?: string;
  /** 启用扩展思考（Claude 3.7+ extended thinking / Qwen3 enable_thinking 等） */
  thinking?: boolean;
  /** Pi-style reasoning depth, mapped to the selected model's supported wire value. */
  thinkingLevel?: ThinkingLevel;
  /** 会话级工作目录（绝对路径），注入 system prompt 并作为 code_agent 默认 cwd */
  cwd?: string;
  requirePermissionApproval?: boolean;
  signal?: AbortSignal;
  onDelta?: (delta: string) => void;
  onAssistantSegmentStart?: (round: number) => void;
  onAssistantSegmentEnd?: (round: number, hasToolCalls: boolean) => void;
  onAssistantSegmentClassified?: (round: number, kind: 'narration' | 'candidate' | 'final') => void;
  onCandidateDisposition?: (round: number, disposition: 'promoted' | 'discarded' | 'superseded') => void;
  onRuntimeUpdate?: (update: { callIndex: number; thinkingLevel: ThinkingLevel }) => void;
  onThinkingDelta?: (delta: string) => void;
  onStreamReset?: (reason: string) => void;
  onToolCall?: (toolCall: { id: string; name: string; arguments: string }) => void;
  onToolCallArgsDelta?: (toolCallId: string, argumentsDelta: string) => void;
  onToolResult?: (result: { toolCallId: string; name: string; output: string; isError: boolean; failure?: import('../types').FailureEnvelope }) => void;
  onToolRecovery?: (result: { toolCallId: string; name: string; phase: string; failure: import('../types').FailureEnvelope }) => void;
  /** 工具执行期实时进度行（包括 code_agent 的 ACP 事件）。 */
  onToolProgress?: (toolCallId: string, line: string) => void;
  /** 工具产出的图片 URL（generate_image 工具）；前端 push 到 assistant 消息的 generatedImages[]。 */
  onToolImage?: (toolCallId: string, url: string, alt: string, record?: GeneratedImageRecord) => void;
  onToolDocument?: (toolCallId: string, document: DocumentRef) => void;
  onToolDecision?: (toolCallId: string, decision: DecisionPromptRecord) => void;
  onDecisionStarted?: (decision: { id: string; trigger: string; risk: string }) => void;
  onDecisionAdvisor?: (decision: { id: string; model: string; status: string; round?: number; summary?: string }) => void;
  onDecisionResolved?: (decision: { id: string; outcome: string; confidence: number; consensus: number; rationale: string }) => void;
  onDecisionEscalated?: (decision: { id: string; rationale: string; options?: DecisionOption[] }) => void;
  onContextUsage?: (usage: ContextUsageSnapshot) => void;
  onDone?: (usage?: TokenUsage) => void;
  onError?: (error: string) => void;
}

export function buildProviderOverride(model?: string, thinkingLevel?: ThinkingLevel) {
  if (!model) return undefined;
  const info = getModelInfo(model);
  if (!info) return undefined;
  return {
    provider_id: info.providerId,
    provider_index: info.providerIndex,
    name: info.providerName,
    model: info.id,
    thinking_level: thinkingLevel,
  };
}

export function useAgentChat() {
  const [isLoading, setIsLoading] = useState(false);
  const abortRef = useRef<AbortController | null>(null);

  const streamMessage = useCallback(
    async (messages: ChatMessage[], options: AgentChatOptions): Promise<void> => {
      setIsLoading(true);
      let signal = options.signal;
      if (!signal) {
        abortRef.current = new AbortController();
        signal = abortRef.current.signal;
      }

      try {
        const res = await fetch(`${getBackendUrl()}/api/agent/sessions/${options.sessionId}/stream`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            messages: messages.map((m) => ({
              role: m.role,
              content: [
                m.content,
                ...(m.collaborationEvidence ?? []).map(evidence => `[${evidence.source}] ${evidence.summary}`),
              ].filter(Boolean).join('\n'),
              images: m.images || [],
              generatedImages: m.generatedImages || [],
              tool_calls: m.tool_calls,
              tool_call_id: m.tool_call_id,
            })),
            system_prompt: options.systemPrompt,
            session_name: options.sessionName,
            provider_override: buildProviderOverride(options.model, options.thinkingLevel),
            thinking: options.thinking ?? false,
            cwd: options.cwd || undefined,
            require_permission_approval: options.requirePermissionApproval ?? false,
          }),
          signal,
        });

        if (!res.ok) {
          const err = await res.json().catch(() => ({ error: 'Unknown error' }));
          options.onError?.(err.error || `HTTP ${res.status}`);
          return;
        }

        const reader = res.body?.getReader();
        if (!reader) {
          options.onError?.('No response body');
          return;
        }

        const decoder = new TextDecoder();
        let buffer = '';
        let currentEvent = '';

        while (true) {
          const { done, value } = await reader.read();
          if (done) break;

          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split('\n');
          buffer = lines.pop() || '';

          for (const line of lines) {
            if (line.startsWith('event: ')) {
              currentEvent = line.slice(7).trim();
              continue;
            }
            if (line.startsWith('data: ')) {
              const data = line.slice(6);
              try {
                const parsed = JSON.parse(data);
                switch (currentEvent) {
                  case 'assistant_segment_start':
                    if (Number.isFinite(parsed.round)) {
                      options.onAssistantSegmentStart?.(parsed.round);
                    }
                    break;
                  case 'text_delta':
                    if (parsed.delta) {
                      options.onDelta?.(parsed.delta);
                    }
                    break;
                  case 'assistant_segment_end':
                    if (Number.isFinite(parsed.round)) {
                      options.onAssistantSegmentEnd?.(parsed.round, !!parsed.has_tool_calls);
                    }
                    break;
                  case 'assistant_segment_classified':
                    if (Number.isFinite(parsed.round) && (
                      parsed.kind === 'narration' || parsed.kind === 'candidate' || parsed.kind === 'final'
                    )) {
                      options.onAssistantSegmentClassified?.(parsed.round, parsed.kind);
                    }
                    break;
                  case 'assistant_candidate_disposition':
                    if (Number.isFinite(parsed.round) && (
                      parsed.disposition === 'promoted' || parsed.disposition === 'discarded' || parsed.disposition === 'superseded'
                    )) {
                      options.onCandidateDisposition?.(parsed.round, parsed.disposition);
                    }
                    break;
                  case 'runtime_update':
                    if (Number.isFinite(parsed.call_index) && typeof parsed.thinking_level === 'string') {
                      options.onRuntimeUpdate?.({
                        callIndex: parsed.call_index,
                        thinkingLevel: parsed.thinking_level as ThinkingLevel,
                      });
                    }
                    break;
                  case 'thinking_delta':
                    if (parsed.delta) {
                      options.onThinkingDelta?.(parsed.delta);
                    }
                    break;
                  case 'stream_reset':
                    options.onStreamReset?.(typeof parsed.reason === 'string' ? parsed.reason : 'repeated stream');
                    break;
                  case 'tool_call_start':
                    if (parsed.id && parsed.name) {
                      options.onToolCall?.({ id: parsed.id, name: parsed.name, arguments: '' });
                    }
                    break;
                  case 'tool_call_delta':
                    if (parsed.id && typeof parsed.arguments_delta === 'string') {
                      options.onToolCallArgsDelta?.(parsed.id, parsed.arguments_delta);
                    }
                    break;
                  case 'tool_result':
                    if (parsed.id && parsed.name && typeof parsed.result === 'string') {
                      options.onToolResult?.({
                        toolCallId: parsed.id,
                        name: parsed.name,
                        output: parsed.result,
                        isError: !!parsed.is_error,
                        failure: parsed.failure && typeof parsed.failure === 'object' ? parsed.failure as import('../types').FailureEnvelope : undefined,
                      });
                    }
                    break;
                  case 'tool_recovery':
                    if (parsed.id && parsed.name && parsed.failure && typeof parsed.failure === 'object') {
                      options.onToolRecovery?.({
                        toolCallId: parsed.id,
                        name: parsed.name,
                        phase: typeof parsed.phase === 'string' ? parsed.phase : 'auto_retrying',
                        failure: parsed.failure as import('../types').FailureEnvelope,
                      });
                    }
                    break;
                  case 'tool_progress':
                    if (parsed.id && typeof parsed.line === 'string') {
                      options.onToolProgress?.(parsed.id, parsed.line);
                    }
                    break;
                  case 'tool_image':
                    if (parsed.id && typeof parsed.url === 'string') {
                      options.onToolImage?.(
                        parsed.id,
                        parsed.url,
                        typeof parsed.alt === 'string' ? parsed.alt : '',
                        parsed.record && typeof parsed.record === 'object' ? parsed.record as GeneratedImageRecord : undefined,
                      );
                    }
                    break;
                  case 'tool_document':
                    if (parsed.id && parsed.document && typeof parsed.document === 'object') {
                      options.onToolDocument?.(parsed.id, parsed.document as DocumentRef);
                    }
                    break;
                  case 'tool_decision':
                    if (parsed.id && parsed.decision && typeof parsed.decision === 'object') {
                      options.onToolDecision?.(parsed.id, parsed.decision as DecisionPromptRecord);
                    }
                    break;
                  case 'decision_started':
                    if (parsed.id) options.onDecisionStarted?.(parsed);
                    break;
                  case 'decision_advisor':
                    if (parsed.id && parsed.model) options.onDecisionAdvisor?.(parsed);
                    break;
                  case 'decision_resolved':
                    if (parsed.id) options.onDecisionResolved?.(parsed);
                    break;
                  case 'decision_escalated':
                    if (parsed.id) options.onDecisionEscalated?.(parsed);
                    break;
                  case 'context_usage':
                    if (Number.isFinite(parsed.prompt_tokens) && Number.isFinite(parsed.completion_tokens)) {
                      options.onContextUsage?.({
                        promptTokens: parsed.prompt_tokens,
                        completionTokens: parsed.completion_tokens,
                        totalTokens: Number.isFinite(parsed.total_tokens)
                          ? parsed.total_tokens
                          : parsed.prompt_tokens + parsed.completion_tokens,
                        callIndex: Number.isFinite(parsed.call_index) ? parsed.call_index : 0,
                        source: parsed.source === 'provider' ? 'provider' : 'estimated',
                        updatedAt: new Date().toISOString(),
                      });
                    }
                    break;
                  case 'done':
                    options.onDone?.(Number.isFinite(parsed.prompt_tokens) ? {
                      promptTokens: parsed.prompt_tokens,
                      completionTokens: parsed.completion_tokens ?? 0,
                      totalTokens: parsed.total_tokens ?? parsed.prompt_tokens + (parsed.completion_tokens ?? 0),
                    } : undefined);
                    requestSyncFromServer('agent-stream');
                    await reader.cancel();
                    return;
                  case 'error':
                    options.onError?.(parsed.message || 'Unknown error');
                    await reader.cancel();
                    return;
                }
              } catch {
                // 非 JSON 数据，忽略
              }
            }
          }
        }

        if (currentEvent !== 'done' && currentEvent !== 'error') {
          options.onDone?.();
          requestSyncFromServer('agent-stream');
        }
      } catch (e) {
        if ((e as Error).name !== 'AbortError') {
          options.onError?.((e as Error).message);
        }
      } finally {
        setIsLoading(false);
        abortRef.current = null;
      }
    },
    []
  );

  const abort = useCallback(() => {
    abortRef.current?.abort();
  }, []);

  return {
    streamMessage,
    abort,
    isLoading,
  };
}

export async function createAgentSession(name: string | undefined, cwd: string): Promise<string> {
  if (!cwd.trim()) throw new Error('创建会话前必须选择工作文件夹');
  const data = await apiFetch<{ id: string }>('/api/agent/sessions', {
    method: 'POST',
    body: JSON.stringify({ name: name || 'default', cwd }),
  });
  return data.id;
}

export interface CompactAgentSessionResult {
  summarized: number;
  kept: number;
  messageCount: number;
  tokensBefore: number;
  tokensAfter: number;
}

export async function compactAgentSession(sessionId: string, messages: ChatMessage[]): Promise<CompactAgentSessionResult> {
  return apiFetch<CompactAgentSessionResult>(
    `/api/agent/sessions/${encodeURIComponent(sessionId)}/compact`,
    {
      method: 'POST',
      body: JSON.stringify({
        messages: messages
          .filter(message => !message.systemNotice)
          .map(message => ({
            role: message.role,
            content: [
              message.content,
              ...(message.collaborationEvidence ?? []).map(evidence => `[${evidence.source}] ${evidence.summary}`),
            ].filter(Boolean).join('\n'),
          })),
      }),
    },
  );
}

export type AgentHarnessAction = 'steer' | 'follow_up' | 'next_turn' | 'abort';

/** Control an in-flight pwcli harness without opening a second chat stream. */
export async function controlAgentHarness(
  sessionId: string,
  action: AgentHarnessAction,
  content?: string,
): Promise<void> {
  await apiFetch(`/api/agent/sessions/${encodeURIComponent(sessionId)}/control`, {
    method: 'POST',
    body: JSON.stringify({ action, ...(content ? { content } : {}) }),
  });
}

export async function updateAgentRuntimeThinking(
  sessionId: string,
  thinkingLevel: ThinkingLevel,
): Promise<void> {
  await apiFetch(`/api/agent/sessions/${encodeURIComponent(sessionId)}/control`, {
    method: 'POST',
    body: JSON.stringify({ action: 'update_runtime', thinking_level: thinkingLevel }),
  });
}

// Module-level cache so multiple ChatInputCard instances share one fetch.
let _skillsCache: AgentSkill[] | null = null;
let _skillsPromise: Promise<AgentSkill[]> | null = null;

async function fetchAgentSkills(): Promise<AgentSkill[]> {
  if (_skillsCache) return _skillsCache;
  if (!_skillsPromise) {
    _skillsPromise = apiFetch<AgentSkill[]>('/api/agent/skills')
      .then((data) => {
        _skillsCache = Array.isArray(data) ? data : [];
        return _skillsCache;
      })
      .catch(() => {
        _skillsPromise = null; // allow retry on next call
        return [];
      });
  }
  return _skillsPromise;
}

/** Hook: load the agent's available skills once (cached across components). */
export function useAgentSkills(): AgentSkill[] {
  const [skills, setSkills] = useState<AgentSkill[]>(_skillsCache || []);
  useEffect(() => {
    if (_skillsCache) {
      setSkills(_skillsCache);
      return;
    }
    let alive = true;
    fetchAgentSkills().then((s) => { if (alive) setSkills(s); });
    return () => { alive = false; };
  }, []);
  return skills;
}
