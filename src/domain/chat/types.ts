import type { WorkspaceRef } from '@/domain/studio';
import type { DocumentRef } from '@/domain/documents';

export type AiModel = string;

export type ChatMode = 'agent' | 'direct';

export interface ChatAttachment {
  type: 'file';
  id: string;
  title: string;
  content: string;
  /** Absolute path, only set for type === 'file'. */
  path?: string;
  /** File size in bytes, only set for type === 'file'. */
  size?: number;
}

export interface MessageStats {
  ttftMs: number;      // Time To First Token
  totalMs: number;     // Total request time
  inputTokens: number; // Estimated input tokens
  outputTokens: number; // Estimated output tokens
}

export interface ChatMessageVersion {
  content: string;
  generatedImages?: string[];
  generatedImageRecords?: GeneratedImageRecord[];
  timestamp: number;
  stats?: MessageStats;
  /** 整个 Agent 回合内所有 LLM 调用的累计用量。 */
  tokenUsage?: TokenUsage;
}

export interface ChatMessage {
  id?: string;
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string;
  images?: string[]; // URLs (persisted to /api/images/) or legacy base64 data URLs
  generatedImages?: string[]; // URLs (persisted to /api/images/) or legacy base64 data URLs
  generatedImageRecords?: GeneratedImageRecord[];
  /** Extended-thinking content streamed alongside the answer (Anthropic
   * thinking_delta / OpenAI reasoning_content). UI shows it in a collapsible
   * block; not sent back to the LLM as conversation context. */
  thinking?: string;
  tool_calls?: { id: string; type: 'function'; function: { name: string; arguments: string } }[];
  tool_call_id?: string;
  /** Error message when the LLM request failed (persisted so it survives reload). */
  error?: string;
  source?: 'background';
  backgroundTaskId?: string;
  /** Live trace of tool calls made by the agent during this assistant turn.
   *  Persisted so it survives reload. args/result are truncated before saving
   *  (see TOOL_TRACE_ARG_LIMIT / TOOL_TRACE_RESULT_LIMIT in store.ts). */
  toolTrace?: ToolTrace[];
  /** Harness-native MoA reviews performed at disputed or risky decision nodes. */
  decisionTrace?: DecisionTrace[];
  /** Structured choice requested by the agent; persisted with the assistant turn. */
  decisionPrompt?: DecisionPromptRecord;
  // Version history for regeneration support
  versions?: ChatMessageVersion[];
  activeVersion?: number; // -1 = use current content (latest), 0+ = index into versions
  stats?: MessageStats;
  /** 整个 Agent 回合内所有 LLM 调用的累计用量。 */
  tokenUsage?: TokenUsage;
  /** Semantic evidence returned by supervised child processes. */
  collaborationEvidence?: CollaborationEvidence[];
  /** One task-level projection for a complete Supervisor run. */
  collaborationTask?: CollaborationTaskSummary;
  /** Structured file/diff references that can be opened without parsing prose. */
  workspaceRefs?: WorkspaceRef[];
  /** Editable HTML documents created or updated by document tools. */
  documentRefs?: DocumentRef[];
  /** Lightweight local/system status that is visible but never sent to the model. */
  systemNotice?: boolean;
}

export type VisualIntent = 'generate' | 'reference' | 'edit' | 'variant';
export type VisualScene = 'auto' | 'photo' | 'illustration' | 'poster' | 'infographic' | 'scientific' | 'product' | 'ui-mockup' | 'asset';
export type ImageReferenceRole = 'edit-target' | 'content' | 'style' | 'composition';

export interface VisualBrief {
  intent: VisualIntent;
  scene: VisualScene;
  prompt: string;
  purpose?: string;
  audience?: string;
  exactText: string[];
  references: Array<{ id: string; role: ImageReferenceRole }>;
  invariants: string[];
  exclusions: string[];
  factualConstraints: string[];
}

export interface GeneratedImageRecord {
  id: string;
  url: string;
  status: 'ready' | 'ready_with_warnings';
  intent: VisualIntent;
  scene: VisualScene;
  visualBrief: VisualBrief;
  finalPrompt: string;
  aspectRatio: string;
  modelId: string;
  providerName: string;
  routingReason?: string;
  references: Array<{ role: ImageReferenceRole; sourceId: string }>;
  qa: {
    status: 'passed' | 'repaired' | 'not-checked' | 'warning';
    issues: Array<{ category: string; description: string }>;
    repairPrompt?: string;
  };
  createdAt: string;
}

export interface CollaborationEvidence {
  id: string;
  childId?: string;
  source: string;
  summary: string;
  status: 'finding' | 'completed' | 'blocked';
}

export interface CollaborationTaskSummary {
  taskId: string;
  title?: string;
  projectName?: string;
  status?: string;
  participants?: string[];
  completedSteps?: number;
  totalSteps?: number;
}

export interface CollaborationState {
  enabled: boolean;
  taskId?: string;
  preferredBackend?: 'auto' | 'codex' | 'qoder' | 'kimi';
  activatedAt?: string;
}

export interface ContextCheckpoint {
  visibleTokensAtCompaction: number;
  effectiveTokens: number;
  compactedAt: string;
}

export interface TokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export interface ContextUsageSnapshot extends TokenUsage {
  callIndex: number;
  source: 'provider' | 'estimated';
  updatedAt: string;
}

export interface ToolTrace {
  id: string;
  name: string;
  args: string;          // 截断到 TOOL_TRACE_ARG_LIMIT
  result?: string;       // 截断到 TOOL_TRACE_RESULT_LIMIT；undefined 表示尚未返回
  isError?: boolean;
  status: 'running' | 'done' | 'error' | 'backgrounded';
  /** 工具执行期实时上抛的进度行（仅长跑工具如 code_agent 有内容）。
   *  pwcli `StreamEvent::ToolProgress` → SSE 'tool_progress' → 这里追加。
   *  上限见 store.ts 的 TOOL_TRACE_PROGRESS_LIMIT。 */
  progressLog?: string[];
}

export interface DecisionTrace {
  id: string;
  trigger: string;
  risk: string;
  status: 'reviewing' | 'resolved' | 'escalated';
  advisors: Array<{ model: string; status: string }>;
  outcome?: string;
  confidence?: number;
  consensus?: number;
  rationale?: string;
  options?: DecisionOption[];
}

export interface DecisionOption {
  id: string;
  label: string;
  description: string;
}

export interface DecisionPromptRecord {
  id: string;
  title: string;
  rationale?: string;
  options: Array<DecisionOption & { recommended?: boolean }>;
  step: number;
  total: number;
  allowCustom: boolean;
  allowSkip: boolean;
}

export interface ChatSession {
  id: string;
  title: string;
  messages: ChatMessage[];
  model: AiModel;
  createdAt: string;
  updatedAt: string;
  /** Legacy virtual folder id; only used during one-time migration. */
  group?: string;
  projectId?: string;
  mode?: ChatMode;
  /** Canonical workspace path bound to this conversation. */
  cwd?: string;
  cwdDisplayPath?: string;
  cwdLockedAt?: string;
  workspaceStatus?: 'ready' | 'missing' | 'denied' | 'awaiting_binding';
  /** Workbench task that owns this AI conversation. */
  taskId?: string;
  /** pwcli backend session ID (sess_xxx format), used to correlate background tasks. */
  agentSessionId?: string;
  /** Durable pwcli collaboration task associated with this chat session. */
  supervisorTaskId?: string;
  /** Deep collaboration augments the current conversation instead of replacing it. */
  collaboration?: CollaborationState;
  /** Server-side materialized context after manual compaction. */
  contextCheckpoint?: ContextCheckpoint;
  /** 最近一次 LLM 调用的实际输入上下文；不是整个 Agent 回合的累计计费用量。 */
  contextUsage?: ContextUsageSnapshot;
}

export interface LocalProject {
  id: string;
  name: string;
  canonicalPath: string;
  displayPath: string;
  availability: 'ready' | 'missing' | 'denied';
  createdAt: string;
  lastOpenedAt: string;
}

/** @deprecated read compatibility for the removed virtual folder model. */
export type ChatFolder = LocalProject;

export type QueuedMessageDelivery = 'next_turn' | 'guidance';
export type QueuedMessageStatus = 'queued' | 'waiting_safe_point' | 'paused' | 'claimed' | 'failed';
export type QueuedMessageSource = 'user' | 'runtime_callback';
export type QueuedMessagePriority = 'normal' | 'high';

export interface QueuedMessage {
  id: string;
  clientMessageId: string;
  sequence: number;
  delivery: QueuedMessageDelivery;
  status: QueuedMessageStatus;
  source?: QueuedMessageSource;
  priority?: QueuedMessagePriority;
  message: {
    role: 'user';
    content: string;
    images?: string[];
  };
  imageUrls?: string[];
  fileReferences?: ChatAttachment[];
  targetTurnId?: string;
  claimedByTurnId?: string;
  createdAt: string;
}

export interface SessionRuntime {
  sessionId: string;
  phase: 'idle' | 'running' | 'paused' | 'stopping' | 'failed';
  activeTurnId?: string;
  paused: boolean;
  queue: QueuedMessage[];
  lastEventSequence: number;
}
