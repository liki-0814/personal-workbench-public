import { lazy, Suspense, useEffect, useState, useCallback, useId, useMemo, useRef } from 'react';
import { createPortal } from 'react-dom';
import { ArrowLeft, ListCollapse, X, RefreshCw, PanelLeftOpen, Activity, Upload, FolderOpen } from 'lucide-react';
import { getSupportedThinkingLevels, preferredThinkingLevel, isVisionModel, THINKING_LEVEL_LABELS, type ThinkingLevel } from '@/core/config/aiProviders';
import { useAiModels } from '@/core/config/hooks';
import { load, save } from '@/core/storage';
import { apiFetch } from '@/core/utils';
import { showToast } from '@/shell';
import type { ChatMessage, AiModel, ChatSession, ChatFolder } from '@/domain/chat';
import { compactAgentSession, useAiChat } from '@/domain/chat';
import { generateId } from '@/core/utils/id';
import {
  ModelSelector,
  ThinkingLevelSelector,
  PermissionModeSelector,
  EmptyChatHero,
  MessageBubble,
  useMessageView,
  useChatSessionBridge,
  BackgroundTaskPanel,
  DecisionPrompt,
} from '../shared';
import { ThinkingIndicator } from '../shared/ThinkingIndicator';
import type { ChatAttachment, DecisionOption, DecisionPromptRecord, DecisionTrace } from '../../types';
import ChatInputCard from './ChatInputCard';
import ChatSidebar from './ChatSidebar';
import DirectoryPickerModal, { type ResolvedDirectory } from './DirectoryPickerModal';
import QueueDock from './QueueDock';
import { DelegateBatchCard } from './DelegationCard';
import CollaborationPanorama from './CollaborationPanorama';
import { isSessionTurnRunning, useSessionRuntime } from '../../state/sessionRuntimeStore';
import { useSessionJournalSync } from '../../state/sessionJournalSync';
import type {
  RuntimeTask,
  RuntimeTaskExecutor,
  RuntimeTaskResult,
  RuntimeTaskReviewAction,
} from '../../state/taskRuntimeStore';
import './precision.css';
import { estimateVisibleContextTokens, getContextWindow } from '../../utils/contextWindow';
import { ContextPane, StudioTab, type ContextPaneState, type WorkspaceRef } from '@/domain/studio';
import type { Goal, TodoItem } from '@/domain/todo';
import { buildGeneratedImageReusePrompt, findLatestGeneratedImageEditInstruction } from '../../utils/generatedImageReuse';
import { importHtmlDocument, type DocumentRef } from '@/domain/documents';
import {
  workItemSection,
  type RuntimeNavigationRequest,
} from '../../utils/workItemProjection';
import { presentRuntimeTask } from './delegationPresentation';

const DocumentWorkspace = lazy(() => import('@/domain/documents/ui/DocumentWorkspace'));

interface Props {
  sessions: ChatSession[];
  activeSessionId: string | null;
  activeSession: ChatSession | null;
  onSelectSession: (id: string) => void;
  onCreateSession: (model?: string, options?: NewSessionOptions) => ChatSession;
  onDeleteSession: (id: string) => void;
  onUpdateSessionMessages: (id: string, messages: ChatMessage[]) => void;
  onUpdateSession: (id: string, patch: Partial<ChatSession>) => void;
  onReorderSessions?: (orderedIds: string[]) => void;
  folders?: ChatFolder[];
  onCreateFolder?: (name: string) => void;
  onRenameFolder?: (id: string, name: string) => void;
  onSetFolderPinned?: (id: string, pinned: boolean) => void;
  onDeleteFolder?: (id: string) => void;
  onMoveSessionToFolder?: (sessionId: string, folderId: string | null) => void;
  onReorderFolders?: (orderedIds: string[]) => void;
  // 后台任务
  bgTasks?: { allTasks: import('../../state/backgroundTaskStore').BackgroundTaskInfo[]; cancel: (id: string) => Promise<void>; cancelBySession: (sid: string) => Promise<void>; dismiss: (id: string) => void; runningCount: number; activeCount: number };
  todos: TodoItem[];
  goals: Goal[];
  onUpdateTodo: (id: string, patch: Partial<TodoItem>) => void;
  onOpenTodo: (id: string) => void;
  isDark: boolean;
  runtimeTasks: RuntimeTask[];
  attentionCounts?: Record<string, number>;
  onCancelRuntimeTask: (taskId: string) => Promise<void>;
  onRetryRuntimeTask: (
    taskId: string,
    executor?: Exclude<RuntimeTaskExecutor, 'auto'>,
  ) => Promise<void>;
  onResolveRuntimeTaskDecision: (taskId: string, option: string, note?: string) => Promise<void>;
  onFollowUpRuntimeTask: (taskId: string, objective: string) => Promise<void>;
  onReviewRuntimeTask: (
    taskId: string,
    action: RuntimeTaskReviewAction,
    reviewRevision: number,
    note?: string,
    projection?: {
      taskUpdate?: RuntimeTaskResult['suggestedTaskUpdate'];
      memoryEntries?: RuntimeTaskResult['decisionCandidates'];
    },
  ) => Promise<void>;
  runtimeNavigation?: RuntimeNavigationRequest;
  onRuntimeNavigationHandled?: (id: number) => void;
}

type NewSessionOptions = Partial<Pick<ChatSession,
  'mode' | 'cwd' | 'taskId'
>>;

interface ConversationTurn {
  userIndex: number;
  question: string;
  answer: string;
}

function cleanTurnText(content: string): string {
  return content
    .replace(/```[\s\S]*?```/g, '代码片段')
    .replace(/[`#>*_~]/g, '')
    .replace(/\[|\]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function buildConversationTurns(messages: ChatMessage[]): ConversationTurn[] {
  const turns: ConversationTurn[] = [];
  for (let index = 0; index < messages.length; index += 1) {
    const message = messages[index];
    if (message.role !== 'user') continue;
    let answer = '';
    for (let next = index + 1; next < messages.length; next += 1) {
      if (messages[next].role === 'user') break;
      if (messages[next].role === 'assistant' && messages[next].content.trim()) {
        answer = cleanTurnText(messages[next].content);
        break;
      }
    }
    turns.push({
      userIndex: index,
      question: cleanTurnText(message.content) || (message.images?.length ? '[图片]' : '未命名问题'),
      answer: answer || '等待回答…',
    });
  }
  return turns;
}

function ConversationIndex({
  turns,
  activeUserIndex,
  onJump,
}: {
  turns: ConversationTurn[];
  activeUserIndex: number | null;
  onJump: (userIndex: number) => void;
}) {
  const [hovered, setHovered] = useState<{ turn: ConversationTurn; left: number; top: number } | null>(null);
  if (turns.length === 0) return null;

  const nearestTurn = (clientY: number, nav: HTMLElement) => {
    const marks = Array.from(nav.querySelectorAll<HTMLButtonElement>('.conversation-index-mark'));
    let nearestIndex = 0;
    let nearestDistance = Number.POSITIVE_INFINITY;
    marks.forEach((mark, index) => {
      const rect = mark.getBoundingClientRect();
      const distance = Math.abs(clientY - (rect.top + rect.height / 2));
      if (distance < nearestDistance) {
        nearestDistance = distance;
        nearestIndex = index;
      }
    });
    return { turn: turns[nearestIndex], mark: marks[nearestIndex] };
  };

  const previewNearest = (clientY: number, nav: HTMLElement) => {
    const nearest = nearestTurn(clientY, nav);
    if (!nearest.turn || !nearest.mark) return;
    const markRect = nearest.mark.getBoundingClientRect();
    const navRect = nav.getBoundingClientRect();
    setHovered({
      turn: nearest.turn,
      left: navRect.right + 10,
      top: Math.max(72, Math.min(window.innerHeight - 230, markRect.top - 78)),
    });
  };

  const hoveredIndex = hovered
    ? turns.findIndex(turn => turn.userIndex === hovered.turn.userIndex)
    : -1;

  return (
    <>
      <nav
        className="conversation-index"
        aria-label="当前对话索引"
        onPointerMove={event => previewNearest(event.clientY, event.currentTarget)}
        onPointerLeave={() => setHovered(null)}
        onClick={event => {
          if ((event.target as HTMLElement).closest('button')) return;
          const nearest = nearestTurn(event.clientY, event.currentTarget);
          if (nearest.turn) onJump(nearest.turn.userIndex);
        }}
      >
        {turns.map((turn, index) => (
          <button
            key={turn.userIndex}
            type="button"
            className={`conversation-index-mark ${hoveredIndex >= 0 ? `is-distance-${Math.min(Math.abs(index - hoveredIndex), 4)}` : ''} ${activeUserIndex === turn.userIndex ? 'is-active' : ''} ${hovered?.turn.userIndex === turn.userIndex ? 'is-nearest' : ''}`}
            onClick={() => onJump(turn.userIndex)}
            onFocus={(event) => {
              const rect = event.currentTarget.getBoundingClientRect();
              const navRect = event.currentTarget.parentElement?.getBoundingClientRect();
              setHovered({
                turn,
                left: (navRect?.right ?? rect.right) + 10,
                top: Math.max(72, Math.min(window.innerHeight - 230, rect.top - 78)),
              });
            }}
            onBlur={() => setHovered(null)}
            aria-label={`跳转到问题：${turn.question}`}
          />
        ))}
      </nav>
      {hovered && createPortal(
        <div
          className="conversation-index-preview"
          style={{ left: hovered.left, top: hovered.top }}
          role="tooltip"
        >
          <div className="conversation-index-question">{hovered.turn.question}</div>
          <div className="conversation-index-answer">{hovered.turn.answer}</div>
        </div>,
        document.body,
      )}
    </>
  );
}
export default function ChatTab({
  sessions,
  activeSessionId,
  activeSession,
  onSelectSession,
  onCreateSession,
  onDeleteSession,
  onUpdateSessionMessages,
  onUpdateSession,
  folders = [],
  onRenameFolder,
  onSetFolderPinned,
  onDeleteFolder,
  bgTasks,
  todos,
  onUpdateTodo,
  onOpenTodo,
  isDark,
  runtimeTasks,
  attentionCounts,
  onCancelRuntimeTask,
  onRetryRuntimeTask,
  onResolveRuntimeTaskDecision,
  onFollowUpRuntimeTask,
  onReviewRuntimeTask,
  runtimeNavigation,
  onRuntimeNavigationHandled,
}: Props) {
  const [bgPanelOpen, setBgPanelOpen] = useState(false);
  const [dismissedTaskContexts, setDismissedTaskContexts] = useState<Set<string>>(() => new Set());
  const bgTaskTriggerRef = useRef<HTMLButtonElement>(null);
  const bgTaskPanelId = useId();

  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => load('chat_sidebar_collapsed', false));
  useEffect(() => {
    if (window.matchMedia('(max-width: 760px)').matches) {
      setSidebarCollapsed(true);
    }
  }, []);
  const [contextPane, setContextPane] = useState<ContextPaneState>({ open: false, mode: 'studio' });
  const [contextReturnDocument, setContextReturnDocument] = useState<DocumentRef>();
  const [contextReturnPanoramaBatchId, setContextReturnPanoramaBatchId] = useState<string>();
  const [contextPaneFocused, setContextPaneFocused] = useState(false);
  const [documentDirty, setDocumentDirty] = useState(false);
  const [documentFocused, setDocumentFocused] = useState(false);
  const documentImportRef = useRef<HTMLInputElement>(null);
  const [studioDirty, setStudioDirty] = useState(false);
  const toggleSidebar = useCallback(() => {
    setSidebarCollapsed((prev: boolean) => {
      const next = !prev;
      save('chat_sidebar_collapsed', next);
      return next;
    });
  }, []);

  // Cmd+\ / Ctrl+\ to toggle sidebar
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === '\\') {
        e.preventDefault();
        toggleSidebar();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [toggleSidebar]);

  const aiModelsForDefault = useAiModels();
  const [draftModel, setDraftModel] = useState<AiModel>(() => aiModelsForDefault[0]?.id ?? '');
  useEffect(() => {
    if (!draftModel && aiModelsForDefault[0]) setDraftModel(aiModelsForDefault[0].id);
  }, [aiModelsForDefault, draftModel]);

  const [inputImages, setInputImages] = useState<string[]>([]);
  const [inputGeneratedImageReferences, setInputGeneratedImageReferences] = useState<string[]>([]);
  const [inputAttachments, setInputAttachments] = useState<ChatAttachment[]>([]);
  const [directoryPicker, setDirectoryPicker] = useState<{
    open: boolean;
    initialPath?: string;
    text: string;
    images?: string[];
    options?: NewSessionOptions;
  }>({ open: false, text: '' });

  useEffect(() => {
    const raw = sessionStorage.getItem('pwb_pending_new_chat');
    if (!raw) return;
    sessionStorage.removeItem('pwb_pending_new_chat');
    try {
      const request = JSON.parse(raw) as { text?: string; initialPath?: string; options?: NewSessionOptions };
      setDirectoryPicker({
        open: true,
        text: request.text ?? '',
        initialPath: request.initialPath,
        options: request.options,
      });
    } catch {
      setDirectoryPicker({ open: true, text: '' });
    }
  }, []);

  const initialMessages = (activeSession?.messages || []).filter(m => m.role !== 'system');

  // Chat mode: agent (full tools + memory) vs direct (pure LLM conversation)
  const linkedTodo = activeSession?.taskId ? todos.find(todo => todo.id === activeSession.taskId) : undefined;

  const handleDeleteSession = useCallback((id: string) => {
    const session = sessions.find(item => item.id === id);
    if (!session) return;
    if ((session.messages.length > 0 || session.agentSessionId)
      && !window.confirm('删除这个会话？将停止当前回答、清除排队消息并取消关联委派；不会删除本地文件。')) return;
    if (!session.agentSessionId) {
      onDeleteSession(id);
      return;
    }
    void apiFetch(`/api/agent/sessions/${encodeURIComponent(session.agentSessionId)}`, { method: 'DELETE' })
      .then(() => onDeleteSession(id))
      .catch(error => showToast({ message: error instanceof Error ? error.message : '后端会话删除失败', type: 'error' }));
  }, [onDeleteSession, sessions]);

  const handleMessagesChange = useCallback((sessionId: string, msgs: ChatMessage[]) => {
    if (sessionId && sessionId !== '_anon') {
      onUpdateSessionMessages(sessionId, msgs);
    }
  }, [onUpdateSessionMessages]);

  const handleAgentSessionCreated = useCallback((agentSid: string, targetSessionId: string) => {
    if (targetSessionId && targetSessionId !== '_anon') {
      onUpdateSession(targetSessionId, { agentSessionId: agentSid });
    }
  }, [onUpdateSession]);

  const handleContextUsage = useCallback((contextUsage: NonNullable<ChatSession['contextUsage']>) => {
    if (activeSessionId) onUpdateSession(activeSessionId, { contextUsage });
  }, [activeSessionId, onUpdateSession]);

  const ai = useAiChat({
    sessionKey: activeSessionId || undefined,
    initialMessages,
    onMessagesChange: handleMessagesChange,
    useAgent: true,
    serverManagedPrompt: true,
    cwd: activeSession?.cwd ?? '',
    initialAgentSessionId: activeSession?.agentSessionId ?? null,
    onAgentSessionCreated: handleAgentSessionCreated,
    onContextUsage: handleContextUsage,
  });
  const sessionRuntime = useSessionRuntime(activeSession?.agentSessionId);
  useSessionJournalSync({
    sessionId: activeSession?.agentSessionId,
    runtime: sessionRuntime.runtime,
    localLoading: ai.loading,
    messages: ai.messages,
    onMerge: ai.replaceMessages,
  });
  const turnRunning = isSessionTurnRunning(sessionRuntime.runtime, ai.loading);
  const sessionRuntimeTasks = useMemo(() => runtimeTasks.filter(task => (
    task.rootSessionId === activeSession?.agentSessionId
    || task.rootSessionId === activeSession?.id
  )), [activeSession?.agentSessionId, activeSession?.id, runtimeTasks]);
  const runningRuntimeTasks = useMemo(
    () => sessionRuntimeTasks.filter(task => workItemSection(task) === 'running'),
    [sessionRuntimeTasks],
  );
  const [focusedRuntimeTaskId, setFocusedRuntimeTaskId] = useState<string>();
  const handleQueueControl = useCallback(async (action: 'resume' | 'send_next' | 'clear_queue') => {
    const response = await sessionRuntime.control(action);
    if (response?.releasedInput?.source === 'runtime_callback') return;
    if (response?.releasedInput?.message.content || response?.releasedMessage?.content) {
      await ai.sendMessage(
        response.releasedInput?.message.content ?? response.releasedMessage!.content,
        response.releasedInput?.imageUrls,
        response.releasedInput?.fileReferences,
      );
    }
  }, [ai, sessionRuntime]);
  const [dismissedDecisionIds, setDismissedDecisionIds] = useState<Set<string>>(() => new Set());
  const [dismissedStructuredDecisionIds, setDismissedStructuredDecisionIds] = useState<Set<string>>(() => new Set());
  const pendingConsensusDecision = useMemo<DecisionTrace | null>(() => {
    if (turnRunning) return null;
    for (let messageIndex = ai.messages.length - 1; messageIndex >= 0; messageIndex -= 1) {
      const message = ai.messages[messageIndex];
      if (message.role === 'user') return null;
      const traces = message.decisionTrace ?? [];
      for (let traceIndex = traces.length - 1; traceIndex >= 0; traceIndex -= 1) {
        const trace = traces[traceIndex];
        if (
          trace.status === 'escalated'
          && (trace.options?.length ?? 0) >= 2
          && !dismissedDecisionIds.has(trace.id)
        ) {
          return trace;
        }
      }
    }
    return null;
  }, [ai.messages, dismissedDecisionIds, turnRunning]);
  const pendingToolDecision = useMemo<DecisionPromptRecord | null>(() => {
    if (turnRunning) return null;
    for (let index = ai.messages.length - 1; index >= 0; index -= 1) {
      const message = ai.messages[index];
      if (message.role === 'user') return null;
      if (message.decisionPrompt && !dismissedStructuredDecisionIds.has(message.decisionPrompt.id)) {
        // code_agent 决策卡片只在它处于最后一条消息时展示：
        // 主 agent 若已自行续聊，中途消息上的旧卡片不应再弹出。
        if (message.decisionPrompt.codeAgentResume && index !== ai.messages.length - 1) continue;
        return message.decisionPrompt;
      }
    }
    return null;
  }, [ai.messages, dismissedStructuredDecisionIds, turnRunning]);

  const resolveConsensusDecision = useCallback(async (trace: DecisionTrace, option: DecisionOption) => {
    setDismissedDecisionIds(current => new Set(current).add(trace.id));
    await ai.sendMessage(
      `我选择 MoA 的「${option.label}」方案。\n\n${option.description}\n\n请严格按这个完整方案继续执行，不要再次评审同一分歧。\n<!-- pwb-moa-user-decision:${trace.id}:${option.id} -->`,
    );
  }, [ai]);

  const resolveConsensusCustom = useCallback(async (trace: DecisionTrace, answer: string) => {
    setDismissedDecisionIds(current => new Set(current).add(trace.id));
    await ai.sendMessage(`${answer}\n\n<!-- pwb-moa-user-decision:${trace.id}:custom -->`);
  }, [ai]);

  const skipConsensusDecision = useCallback(async (trace: DecisionTrace) => {
    setDismissedDecisionIds(current => new Set(current).add(trace.id));
    await ai.sendMessage(`跳过本次方案选择，请按你认为最稳妥的方式继续。\n<!-- pwb-moa-user-decision:${trace.id}:skip -->`);
  }, [ai]);

  const resolveCodeAgentDecision = useCallback(async (decision: DecisionPromptRecord, message: string) => {
    const resumeSessionId = decision.codeAgentResume?.sessionId;
    if (!resumeSessionId) return;
    setDismissedStructuredDecisionIds(ids => new Set(ids).add(decision.id));
    try {
      const agentSessionId = activeSession?.agentSessionId ?? await ai.ensureAgentSession();
      if (!agentSessionId) throw new Error('没有可用的会话');
      await apiFetch(`/api/agent/sessions/${encodeURIComponent(agentSessionId)}/code-agent/decision`, {
        method: 'POST',
        body: JSON.stringify({ sessionId: resumeSessionId, message }),
      });
      showToast({ message: '已在后台续聊子 agent，完成后自动通知', type: 'success' });
    } catch (error) {
      showToast({ message: error instanceof Error ? error.message : '续聊子 agent 失败', type: 'error' });
    }
  }, [activeSession?.agentSessionId, ai]);


  const openStudio = useCallback((workspaceRef: WorkspaceRef) => {
    setContextReturnDocument(undefined);
    setContextReturnPanoramaBatchId(undefined);
    setContextPaneFocused(false);
    setContextPane({
      open: true,
      mode: 'studio',
      supervisorTaskId: workspaceRef.supervisorTaskId,
      reviewRevision: workspaceRef.reviewRevision,
      workspaceRef,
    });
  }, []);

  const openDocument = useCallback((documentRef: DocumentRef) => {
    setContextReturnDocument(undefined);
    setContextReturnPanoramaBatchId(undefined);
    setDocumentDirty(false);
    setContextPane({ open: true, mode: 'document', documentRef });
  }, []);

  const openCollaborationPanorama = useCallback((batchId: string) => {
    setContextReturnDocument(undefined);
    setContextReturnPanoramaBatchId(undefined);
    setDocumentDirty(false);
    setDocumentFocused(false);
    setContextPaneFocused(false);
    setContextPane({ open: true, mode: 'collaboration', runtimeBatchId: batchId });
  }, []);

  const taskDocumentRef = useCallback((task: RuntimeTask, documentId?: string, title?: string): DocumentRef | undefined => {
    const view = presentRuntimeTask(task);
    const id = documentId || view.documentId;
    if (!id) return undefined;
    return {
      id,
      kind: 'markdown',
      title: title || view.documentTitle,
      revision: 0,
      status: 'ready',
      qaStatus: 'pending',
      runtime: 'delegated',
    };
  }, []);

  const openTaskDocument = useCallback((documentId: string, task: RuntimeTask, title?: string) => {
    const documentRef = taskDocumentRef(task, documentId, title);
    if (documentRef) openDocument(documentRef);
  }, [openDocument, taskDocumentRef]);

  const openTaskDiff = useCallback((workspaceRef: WorkspaceRef, task: RuntimeTask) => {
    openStudio(workspaceRef);
    setContextReturnDocument(taskDocumentRef(task));
  }, [openStudio, taskDocumentRef]);

  const openPanoramaDocument = useCallback((documentId: string, task: RuntimeTask, title?: string) => {
    const batchId = contextPane.runtimeBatchId;
    if (!batchId) return;
    openTaskDocument(documentId, task, title);
    setContextReturnPanoramaBatchId(batchId);
  }, [contextPane.runtimeBatchId, openTaskDocument]);

  const openPanoramaDiff = useCallback((workspaceRef: WorkspaceRef, _task: RuntimeTask) => {
    const batchId = contextPane.runtimeBatchId;
    if (!batchId) return;
    openStudio(workspaceRef);
    setContextReturnDocument(undefined);
    setContextReturnPanoramaBatchId(batchId);
  }, [contextPane.runtimeBatchId, openStudio]);

  useEffect(() => {
    if (!runtimeNavigation) return;
    if (runtimeNavigation.sessionId && runtimeNavigation.sessionId !== activeSession?.id) return;
    const task = sessionRuntimeTasks.find(candidate => candidate.id === runtimeNavigation.taskId);
    if (!task) return;
    setFocusedRuntimeTaskId(task.id);
    if (runtimeNavigation.target.kind === 'document') {
      openTaskDocument(runtimeNavigation.target.documentId, task);
    } else if (runtimeNavigation.target.kind === 'diff') {
      const path = runtimeNavigation.target.path;
      const file = presentRuntimeTask(task).changedFiles.find(candidate => candidate.path === path);
      openTaskDiff({
        path,
        kind: 'diff',
        taskId: task.id,
        workspaceRoot: task.cwd,
        reviewRevision: task.reviewRevision,
        inlinePatch: file?.patch,
      }, task);
    } else {
      window.requestAnimationFrame(() => {
        const element = document.getElementById(`delegation-task-${task.id}`);
        element?.scrollIntoView({ behavior: 'smooth', block: 'center' });
        element?.focus({ preventScroll: true });
      });
    }
    onRuntimeNavigationHandled?.(runtimeNavigation.id);
  }, [activeSession?.id, onRuntimeNavigationHandled, openTaskDiff, openTaskDocument, runtimeNavigation, sessionRuntimeTasks]);

  const importDocument = useCallback(async (file: File) => {
    try {
      const imported = await importHtmlDocument(file);
      openDocument(imported.manifest);
      showToast({ message: `已导入「${imported.manifest.title}」`, type: 'success' });
    } catch (reason) {
      showToast({ message: reason instanceof Error ? reason.message : '文档导入失败', type: 'error' });
    }
  }, [openDocument]);


  const closeContextPane = useCallback(() => {
    if (contextPane.mode === 'studio' && studioDirty && !window.confirm('Studio 中有未保存的修改，仍要关闭吗？')) return;
    if (contextPane.mode === 'document' && documentDirty && !window.confirm('文档中有未保存的修改，仍要关闭吗？')) return;
    if (contextPane.mode === 'studio') setStudioDirty(false);
    if (contextPane.mode === 'document') {
      setDocumentDirty(false);
      setDocumentFocused(false);
    }
    setContextReturnDocument(undefined);
    setContextReturnPanoramaBatchId(undefined);
    setContextPaneFocused(false);
    setContextPane(current => ({ ...current, open: false }));
  }, [contextPane.mode, documentDirty, studioDirty]);


  const sendPendingMessage = useCallback(async (text: string, images: string[] | undefined) => {
    await ai.sendMessage(text, images);
  }, [ai]);

  const { pendingFirstMessageRef } = useChatSessionBridge({
    activeSession,
    activeSessionId,
    ai,
    onUpdateSession,
    onPendingMessage: sendPendingMessage,
  });

  const currentModel = activeSession ? ai.model : draftModel;
  const contextUsage = useMemo(() => {
    const visibleTokens = estimateVisibleContextTokens(ai.messages.filter(message => !message.systemNotice));
    const checkpoint = activeSession?.contextCheckpoint;
    const providerUsage = activeSession?.contextUsage;
    return {
      usedTokens: providerUsage
        ? providerUsage.promptTokens + providerUsage.completionTokens
        : checkpoint
        ? checkpoint.effectiveTokens + Math.max(0, visibleTokens - checkpoint.visibleTokensAtCompaction)
        : visibleTokens,
      exactUsage: providerUsage?.source === 'provider',
      ...getContextWindow(currentModel),
    };
  }, [activeSession?.contextCheckpoint, activeSession?.contextUsage, ai.messages, currentModel]);
  const acceptsImages = isVisionModel(currentModel);
  const thinkingLevels = useMemo(() => getSupportedThinkingLevels(currentModel), [currentModel]);
  const thinkingClampNoticeRef = useRef('');
  const setCurrentThinkingLevel = useCallback((level: ThinkingLevel) => {
    save(`thinking_level:${currentModel}`, level);
    void ai.setThinkingLevelForNextCall(level).catch((error: unknown) => {
      showToast({
        message: error instanceof Error ? error.message : '思考强度更新失败',
        type: 'error',
      });
    });
  }, [ai, currentModel]);

  useEffect(() => {
    if (!acceptsImages && inputImages.length > 0) {
      setInputImages([]);
      showToast({ message: '当前模型不支持图片输入，已清空待发送图片', type: 'info' });
    }
  }, [acceptsImages, inputImages.length]);

  const composerImages = useMemo(
    () => [...inputImages, ...inputGeneratedImageReferences],
    [inputGeneratedImageReferences, inputImages],
  );

  const handleComposerImagesChange = useCallback((next: string[]) => {
    setInputGeneratedImageReferences(current => current.filter(url => next.includes(url)));
    setInputImages(next.filter(url => !inputGeneratedImageReferences.includes(url)));
  }, [inputGeneratedImageReferences]);

  // Pi-style: persist one preferred level, but clamp it to what the selected
  // model explicitly declares instead of sending unsupported provider values.
  useEffect(() => {
    const saved = load<ThinkingLevel | null>(`thinking_level:${currentModel}`, null);
    const next = saved && thinkingLevels.includes(saved)
      ? saved
      : thinkingLevels.includes(ai.thinkingLevel)
        ? ai.thinkingLevel
        : preferredThinkingLevel(thinkingLevels);
    if (next !== ai.thinkingLevel) {
      ai.setThinkingLevel(next);
    }
    if (saved && !thinkingLevels.includes(saved)) {
      const noticeKey = `${currentModel}:${saved}:${next}`;
      if (thinkingClampNoticeRef.current !== noticeKey) {
        thinkingClampNoticeRef.current = noticeKey;
        showToast({
          message: `当前模型不支持“${THINKING_LEVEL_LABELS[saved]}”，已切换为“${THINKING_LEVEL_LABELS[next]}”`,
          type: 'info',
        });
      }
    }
  }, [ai, currentModel, thinkingLevels]);

  const {
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
  } = useMessageView({ messages: ai.messages, loading: turnRunning });

  const conversationTurns = useMemo(
    () => buildConversationTurns(ai.messages),
    [ai.messages],
  );
  const lastConversationTurnIndex = conversationTurns.length > 0
    ? conversationTurns[conversationTurns.length - 1].userIndex
    : null;
  const [activeTurnUserIndex, setActiveTurnUserIndex] = useState<number | null>(null);

  useEffect(() => {
    setActiveTurnUserIndex(lastConversationTurnIndex);
  }, [activeSessionId, lastConversationTurnIndex]);

  const handleThreadScroll = useCallback(() => {
    handleScroll();
    const container = scrollRef.current;
    if (!container) return;
    const anchors = Array.from(container.querySelectorAll<HTMLElement>('[data-conversation-turn]'));
    if (anchors.length === 0) return;
    const threshold = container.getBoundingClientRect().top + 120;
    let active = Number(anchors[0].dataset.conversationTurn);
    for (const anchor of anchors) {
      if (anchor.getBoundingClientRect().top > threshold) break;
      active = Number(anchor.dataset.conversationTurn);
    }
    setActiveTurnUserIndex(active);
  }, [handleScroll, scrollRef]);

  const jumpToTurn = useCallback((userIndex: number) => {
    showAll();
    setActiveTurnUserIndex(userIndex);
    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => {
        document.getElementById(`chat-turn-${userIndex}`)?.scrollIntoView({
          behavior: 'smooth',
          block: 'start',
        });
      });
    });
  }, [showAll]);

  // 空输入框按 ↑ 调出的上一条用户消息（取最近一条 role==='user'）
  const lastUserText = useMemo(() => {
    for (let i = ai.messages.length - 1; i >= 0; i--) {
      if (ai.messages[i].role === 'user') return ai.messages[i].content;
    }
    return '';
  }, [ai.messages]);

  /**
   * Open a new session and queue the first message so it auto-fires once the
   * session id flips.
   */
  const startNewSessionWithDraft = useCallback((overridePrompt?: string, initialPath?: string) => {
    const override = (overridePrompt ?? '').trim();
    setDirectoryPicker({
      open: true,
      initialPath,
      text: override,
    });
  }, []);

  const createSessionInProject = useCallback((initialPath?: string) => {
    if (!initialPath) {
      startNewSessionWithDraft();
      return;
    }
    ai.setModel(draftModel);
    onCreateSession(draftModel, { mode: 'agent', cwd: initialPath });
    window.requestAnimationFrame(() => {
      document.querySelector<HTMLTextAreaElement>('textarea')?.focus();
    });
  }, [ai, draftModel, onCreateSession, startNewSessionWithDraft]);

  const confirmNewSessionDirectory = useCallback((directory: ResolvedDirectory) => {
    if (directoryPicker.text || (directoryPicker.images?.length ?? 0) > 0) {
      pendingFirstMessageRef.current = {
        text: directoryPicker.text,
        images: directoryPicker.images,
        collaboration: false,
      };
    }
    ai.setModel(draftModel);
    const created = onCreateSession(draftModel, {
      ...directoryPicker.options,
      mode: 'agent',
      cwd: directory.canonicalPath,
    });
    if (directoryPicker.options?.taskId) {
      const todo = todos.find(item => item.id === directoryPicker.options?.taskId);
      if (todo) {
        onUpdateTodo(todo.id, {
          linkedChatSessionIds: [...new Set([...(todo.linkedChatSessionIds ?? []), created.id])],
        });
      }
    }
    setInputImages([]);
    setDirectoryPicker({ open: false, text: '' });
  }, [ai, directoryPicker.images, directoryPicker.options, directoryPicker.text, draftModel, onCreateSession, onUpdateTodo, pendingFirstMessageRef, todos]);

  const [compacting, setCompacting] = useState(false);
  const handleCompact = useCallback(async () => {
    if (!activeSessionId || compacting) return;
    setCompacting(true);
    try {
      const sessionId = await ai.ensureAgentSession();
      const result = await compactAgentSession(sessionId, ai.messages);
      onUpdateSession(activeSessionId, {
        contextCheckpoint: {
          visibleTokensAtCompaction: estimateVisibleContextTokens(ai.messages),
          effectiveTokens: result.tokensAfter,
          compactedAt: new Date().toISOString(),
        },
        contextUsage: undefined,
      });
      ai.replaceMessages([
        ...ai.messages,
        {
          id: generateId(),
          role: 'assistant',
          systemNotice: true,
          content: `上下文已压缩：约 ${result.tokensBefore.toLocaleString()} → ${result.tokensAfter.toLocaleString()} tokens；完整历史仍可查看。`,
        },
      ]);
      ai.setInput('');
      showToast({
        message: `上下文已压缩：约 ${result.tokensBefore.toLocaleString()} → ${result.tokensAfter.toLocaleString()} tokens`,
        type: 'success',
      });
    } catch (reason) {
      showToast({ message: reason instanceof Error ? reason.message : '上下文压缩失败', type: 'error' });
    } finally {
      setCompacting(false);
    }
  }, [activeSessionId, ai, compacting, onUpdateSession]);


  return (
    <div className="precision-chat pwb-chat-layout">
      <input ref={documentImportRef} type="file" accept=".html,.htm,text/html" hidden onChange={event => {
        const file = event.target.files?.[0];
        if (file) void importDocument(file);
        event.target.value = '';
      }} />
      {/* Rail: visible when sidebar collapsed */}
      {sidebarCollapsed && (
        <button
          type="button"
          className="pwb-sidebar-rail"
          onClick={toggleSidebar}
          title="展开侧栏 (⌘\)"
          aria-label="展开会话侧栏"
        >
          <PanelLeftOpen size={16} />
        </button>
      )}

      {/* Sidebar */}
      <ChatSidebar
        sessions={sessions}
        projects={folders}
        activeSessionId={activeSessionId}
        onSelectSession={onSelectSession}
        onCreateSession={createSessionInProject}
        onDeleteSession={handleDeleteSession}
        onUpdateSession={onUpdateSession}
        onRenameProject={onRenameFolder}
        onSetProjectPinned={onSetFolderPinned}
        onRemoveProject={onDeleteFolder}
        collapsed={sidebarCollapsed}
        onToggleCollapse={toggleSidebar}
        attentionCounts={attentionCounts}
        runtimes={activeSession?.agentSessionId && sessionRuntime.runtime
          ? { [activeSession.agentSessionId]: sessionRuntime.runtime }
          : {}}
      />

      {/* Main */}
      <div className="pwb-main precision-chat-main">
        {!activeSession ? (
          <div className="h-full flex flex-col">
            {/* New-session execution toolbar */}
            <div className="pwb-toolbar">
              <div className="precision-toolbar-title">
                <span className="precision-toolbar-eyebrow">AI</span>
                <span>新会话</span>
              </div>
              <div className="precision-toolbar-controls">
                <div className="precision-toolbar-group">
                  <ModelSelector value={draftModel} onChange={setDraftModel} size="sm" />
                  <ThinkingLevelSelector
                    value={ai.thinkingLevel}
                    levels={thinkingLevels}
                    onChange={setCurrentThinkingLevel}
                  />
                  <PermissionModeSelector />
                </div>
                <div className="precision-toolbar-group precision-toolbar-actions">
                  <button
                    type="button"
                    disabled
                    className="precision-icon-button opacity-40"
                    title="暂无对话可压缩"
                    aria-label="暂无对话可压缩"
                  >
                    <ListCollapse size={15} />
                  </button>
                </div>
              </div>
            </div>

            {/* Welcome hero + prompt cards */}
            <div className="precision-empty-stage flex-1 overflow-y-auto">
              <EmptyChatHero
                title="开始新的工作会话"
                subtitle="直接描述目标，或从常用任务开始"
                prompts={[
                  { text: '帮我写一段代码', hint: '生成、解释或调试代码' },
                  { text: '总结今天的任务', hint: '梳理待办与日程' },
                  { text: '推荐几个学习网站', hint: '按方向给出资源清单' },
                  { text: '帮我添加一个待办', hint: '自然语言转结构化任务' },
                ]}
                onPick={prompt => startNewSessionWithDraft(prompt)}
              />
            </div>

            {/* Input */}
            <div className="precision-composer-wrap w-full">
              <ChatInputCard
                value={ai.input}
                onChange={ai.setInput}
                onSubmit={async () => {
                  if (!ai.input.trim() && inputImages.length === 0) return;
                  startNewSessionWithDraft();
                }}
                images={inputImages}
                onImagesChange={setInputImages}
                acceptsImages={acceptsImages}
                attachments={inputAttachments}
                onAttachmentsChange={setInputAttachments}
                sessionMode="agent"
                cwd={undefined}
                contextUsage={contextUsage}
              />
            </div>
          </div>
        ) : (
          <>
            {/* Session execution toolbar */}
            <div className="pwb-toolbar">
              <div className="precision-toolbar-title min-w-0">
                <span className="precision-toolbar-eyebrow">AI</span>
                <span className="truncate">
                  {activeSession.title}
                </span>
              </div>
              <div className="precision-toolbar-controls">
                <div className="precision-toolbar-group">
                  <ModelSelector
                    value={ai.model}
                    onChange={m => {
                      ai.setModel(m);
                      if (activeSessionId) onUpdateSession(activeSessionId, { model: m, contextUsage: undefined });
                    }}
                    size="sm"
                  />
                  <ThinkingLevelSelector
                    value={ai.thinkingLevel}
                    levels={thinkingLevels}
                    onChange={setCurrentThinkingLevel}
                    pending={ai.pendingThinkingLevel !== null}
                  />
                  <PermissionModeSelector />
                </div>
                <div className="precision-toolbar-group precision-toolbar-actions">
                  {bgTasks && bgTasks.activeCount > 0 && (
                    <div className="relative">
                    <button
                      ref={bgTaskTriggerRef}
                      type="button"
                      onClick={() => setBgPanelOpen(v => !v)}
                      className="pwb-pill-btn precision-tool-button pwb-pill-btn-active"
                      title={`${bgTasks.runningCount} 个运行中 / ${bgTasks.activeCount} 个总计`}
                      aria-expanded={bgPanelOpen}
                      aria-haspopup="dialog"
                      aria-controls={bgPanelOpen ? bgTaskPanelId : undefined}
                    >
                      <Activity size={13} className="animate-pulse" />
                      <span>{bgTasks.activeCount}</span>
                    </button>
                    {bgPanelOpen && (
                      <BackgroundTaskPanel
                        id={bgTaskPanelId}
                        anchorRef={bgTaskTriggerRef}
                        tasks={bgTasks.allTasks}
                        onCancel={bgTasks.cancel}
                        onDismiss={bgTasks.dismiss}
                        onClose={() => setBgPanelOpen(false)}
                      />
                    )}
                    </div>
                  )}
                  <button
                    type="button"
                    onClick={() => activeSession.cwd && openStudio({
                      path: '',
                      kind: 'file',
                      taskId: activeSession.taskId,
                      workspaceRoot: activeSession.cwd,
                    })}
                    disabled={!activeSession.cwd}
                    className="precision-icon-button"
                    title="在第三屏打开工作文件夹"
                    aria-label="打开工作文件夹"
                  >
                    <FolderOpen size={15} />
                  </button>
                  <button
                    type="button"
                    onClick={() => documentImportRef.current?.click()}
                    className="precision-icon-button"
                    title="导入 HTML"
                    aria-label="导入 HTML 文档"
                  >
                    <Upload size={15} />
                  </button>
                  <button
                    type="button"
                    onClick={() => void handleCompact()}
                    disabled={compacting || turnRunning || !activeSession.cwd}
                    className="precision-icon-button"
                    title={!activeSession.cwd ? '绑定工作文件夹后才能压缩上下文' : compacting ? '正在压缩上下文' : '压缩上下文'}
                    aria-label={!activeSession.cwd ? '绑定工作文件夹后才能压缩上下文' : compacting ? '正在压缩上下文' : '压缩上下文'}
                  >
                    {compacting ? <RefreshCw size={15} className="animate-spin" /> : <ListCollapse size={15} />}
                  </button>
                </div>
              </div>
            </div>

            {linkedTodo && !dismissedTaskContexts.has(activeSession.id) && (
              <div className="task-context-strip" role="status">
                <span className="task-context-copy">
                  <small>来自任务</small>
                  <strong>{linkedTodo.title}</strong>
                </span>
                <span className="task-context-actions">
                  <button type="button" onClick={() => onOpenTodo(linkedTodo.id)}>
                    <ArrowLeft size={13} />
                    返回任务
                  </button>
                  <button
                    type="button"
                    onClick={() => setDismissedTaskContexts(current => new Set(current).add(activeSession.id))}
                    aria-label="关闭任务来源提示"
                  >
                    <X size={14} />
                  </button>
                </span>
              </div>
            )}

            {/* Messages — centered column */}
            <ConversationIndex
              turns={conversationTurns}
              activeUserIndex={activeTurnUserIndex}
              onJump={jumpToTurn}
            />
            <div
              ref={scrollRef}
              onScroll={handleThreadScroll}
              className="precision-message-scroll flex-1 overflow-y-auto overflow-x-hidden min-h-0"
            >
              <div className="precision-message-column">
                {visibleMessageEntries.length === 0 && (
                  <div className="precision-thread-empty h-full flex flex-col items-center justify-center py-16">
                    <div className="precision-agent-mark mb-3" aria-hidden>AI</div>
                    <p className="text-sm">发送消息开始对话</p>
                  </div>
                )}

                {hasMore && (
                  <div className="flex justify-center py-3">
                    <button
                      onClick={loadMore}
                      className="precision-secondary-button"
                    >
                      加载更早的消息
                    </button>
                  </div>
                )}

                {visibleMessageEntries.map(({ msg, idx: originalIdx }) => {
                  const msgKey = messageKey(msg, originalIdx);
                  return (
                    <div
                      key={msg.id || originalIdx}
                      id={msg.role === 'user' ? `chat-turn-${originalIdx}` : undefined}
                      data-conversation-turn={msg.role === 'user' ? originalIdx : undefined}
                      className="conversation-message-anchor"
                    >
                      <MessageBubble
                        msg={msg}
                        isLastAssistant={lastAssistantOriginalIdx === originalIdx}
                        loading={turnRunning}
                        expanded={expandedToolCalls.has(msgKey)}
                        onToggleExpand={() => toggleToolExpand(msgKey)}
                        copied={copiedMsgKey === msgKey}
                        onCopy={text => copyMessage(text, msgKey)}
                        onRegenerate={() => ai.regenerate()}
                        onSwitchVersion={v => ai.switchVersion(originalIdx, v)}
                        onOpenWorkspaceRef={openStudio}
                        onOpenDocument={openDocument}
                        onReuseGeneratedImage={(record, mode) => {
                          const historyInstruction = findLatestGeneratedImageEditInstruction(ai.messages, originalIdx);
                          ai.setInput(buildGeneratedImageReusePrompt(record, mode, historyInstruction));
                          setInputGeneratedImageReferences([record.url]);
                          window.requestAnimationFrame(() => document.querySelector<HTMLTextAreaElement>('textarea')?.focus());
                        }}
                      />
                    </div>
                  );
                })}

                {turnRunning && lastAssistantOriginalIdx === -1 && (
                  <div className="flex gap-3 justify-start animate-fade-in-up">
                    <div className="precision-agent-mark is-streaming" aria-hidden>AI</div>
                    <div className="mt-1.5">
                      <ThinkingIndicator />
                    </div>
                  </div>
                )}

                {ai.error && (
                  <div className="precision-error-banner flex items-center gap-2 px-4 py-2.5 text-xs">
                    <span className="flex-1">{ai.error}</span>
                    <button
                      onClick={() => ai.retry()}
                      disabled={turnRunning}
                      className="precision-error-action flex items-center gap-1 px-2.5 py-1 disabled:opacity-50"
                    >
                      <RefreshCw size={12} />
                      重试
                    </button>
                    <button
                      onClick={() => ai.clearError()}
                      className="precision-error-close p-1"
                      title="关闭"
                    >
                      <X size={12} />
                    </button>
                  </div>
                )}

                {sessionRuntimeTasks.length > 0 && (
                  <div className="delegation-timeline-entry" aria-label="协作者进度">
                    <DelegateBatchCard
                      tasks={sessionRuntimeTasks}
                      onCancel={onCancelRuntimeTask}
                      onRetry={onRetryRuntimeTask}
                      onResolveDecision={onResolveRuntimeTaskDecision}
                      onFollowUp={onFollowUpRuntimeTask}
                      onReview={onReviewRuntimeTask}
                      onOpenFile={openStudio}
                      onOpenDocument={(documentId, task, title) => openTaskDocument(documentId, task, title)}
                      onOpenDiff={openTaskDiff}
                      onOpenPanorama={openCollaborationPanorama}
                      focusTaskId={focusedRuntimeTaskId}
                    />
                  </div>
                )}
              </div>
            </div>

            {/* Input — floating card centered */}
            <div className="precision-composer-wrap w-full">
              {runningRuntimeTasks.length > 0 && (
                <button
                  type="button"
                  className="delegation-dock-status"
                  onClick={() => {
                    const task = runningRuntimeTasks[0];
                    setFocusedRuntimeTaskId(task.id);
                    window.requestAnimationFrame(() => {
                      document.getElementById(`delegation-task-${task.id}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' });
                    });
                  }}
                  aria-label={`查看 ${runningRuntimeTasks.length} 个后台运行任务`}
                >
                  <Activity size={13} className="delegation-spin" />
                  <span>{runningRuntimeTasks.length} 位协作者后台运行</span>
                </button>
              )}
              {sessionRuntime.runtime && sessionRuntime.runtime.queue.length > 0 && (
                <QueueDock
                  runtime={sessionRuntime.runtime}
                  onUpdate={sessionRuntime.updateItem}
                  onDelete={sessionRuntime.deleteItem}
                  onControl={handleQueueControl}
                />
              )}
              {pendingToolDecision ? <DecisionPrompt
                title={pendingToolDecision.title}
                rationale={pendingToolDecision.rationale}
                step={pendingToolDecision.step}
                total={pendingToolDecision.total}
                options={pendingToolDecision.options.map((option, index) => ({ ...option, recommended: option.recommended ?? index === 0 }))}
                onSelect={option => pendingToolDecision.codeAgentResume
                  ? void resolveCodeAgentDecision(pendingToolDecision, `选择「${option.label}」${option.description ? `：${option.description}` : ''}`)
                  : ai.sendMessage(`我选择「${option.label}」。\n\n${option.description ?? ''}\n<!-- pwb-user-choice:${pendingToolDecision.id}:${option.id} -->`)}
                onCustom={pendingToolDecision.allowCustom ? answer => pendingToolDecision.codeAgentResume
                  ? void resolveCodeAgentDecision(pendingToolDecision, answer)
                  : ai.sendMessage(`${answer}\n<!-- pwb-user-choice:${pendingToolDecision.id}:custom -->`) : undefined}
                onSkip={pendingToolDecision.allowSkip ? () => ai.sendMessage(`跳过这个问题，请按推荐选项继续。\n<!-- pwb-user-choice:${pendingToolDecision.id}:skip -->`) : undefined}
                onClose={() => setDismissedStructuredDecisionIds(ids => new Set(ids).add(pendingToolDecision.id))}
              /> : pendingConsensusDecision ? <DecisionPrompt
                title="需要你选择继续方案"
                rationale={pendingConsensusDecision.rationale}
                options={(pendingConsensusDecision.options ?? []).map((option, index) => ({ ...option, recommended: index === 0 }))}
                onSelect={option => resolveConsensusDecision(pendingConsensusDecision, { id: option.id, label: option.label, description: option.description ?? '' })}
                onCustom={answer => resolveConsensusCustom(pendingConsensusDecision, answer)}
                onSkip={() => skipConsensusDecision(pendingConsensusDecision)}
                onClose={() => setDismissedDecisionIds(ids => new Set(ids).add(pendingConsensusDecision.id))}
              /> : <ChatInputCard
                value={ai.input}
                onChange={ai.setInput}
                onSubmit={async () => {
                  if (!ai.input.trim() && composerImages.length === 0 && inputAttachments.length === 0) return;
                  if (turnRunning) {
                    try {
                      await sessionRuntime.enqueue(ai.input.trim(), 'next_turn', composerImages, inputAttachments);
                      ai.setInput('');
                      setInputAttachments([]);
                      setInputImages([]);
                      setInputGeneratedImageReferences([]);
                    } catch (error) {
                      showToast({ message: error instanceof Error ? error.message : '消息排队失败', type: 'error' });
                    }
                    return;
                  }
                  if (ai.input.trim() === '/compact') {
                    await handleCompact();
                    return;
                  }
                  ai.sendMessage(
                    ai.input,
                    inputImages,
                    inputAttachments.length > 0 ? inputAttachments : undefined,
                    inputGeneratedImageReferences,
                  );
                  setInputAttachments([]);
                  setInputImages([]);
                  setInputGeneratedImageReferences([]);
                }}
                onGuide={async () => {
                  if (!ai.input.trim() || pendingToolDecision || pendingConsensusDecision) return;
                  try {
                    await sessionRuntime.enqueue(ai.input.trim(), 'guidance', composerImages, inputAttachments);
                    ai.setInput('');
                    setInputAttachments([]);
                    setInputImages([]);
                    setInputGeneratedImageReferences([]);
                  } catch (error) {
                    showToast({ message: error instanceof Error ? error.message : '引导发送失败', type: 'error' });
                  }
                }}
                images={composerImages}
                onImagesChange={handleComposerImagesChange}
                acceptsImages={acceptsImages}
                streaming={turnRunning}
                onStop={ai.stop}
                attachments={inputAttachments}
                onAttachmentsChange={setInputAttachments}
                lastUserText={lastUserText}
                sessionMode="agent"
                cwd={activeSession.cwd}
                onOpenWorkspace={() => activeSession.cwd && openStudio({
                  path: '',
                  kind: 'file',
                  taskId: activeSession.taskId,
                  workspaceRoot: activeSession.cwd,
                })}
                contextUsage={contextUsage}
              />}
            </div>
          </>
        )}
      </div>

      {contextPane.open && (
        <ContextPane
          mode={contextPane.mode}
          title={contextPane.mode === 'document'
            ? (contextPane.documentRef?.title || (contextPane.documentRef?.kind === 'markdown' ? '协作者文档' : 'HTML 文档工作台'))
            : contextPane.mode === 'collaboration' ? '协作全景图' : '项目文件'}
          onClose={closeContextPane}
          onBack={contextReturnPanoramaBatchId && ['studio', 'document'].includes(contextPane.mode)
            ? () => openCollaborationPanorama(contextReturnPanoramaBatchId)
            : contextPane.mode === 'studio' && contextReturnDocument ? () => {
                const documentRef = contextReturnDocument;
                setContextReturnDocument(undefined);
                openDocument(documentRef);
              } : undefined}
          focused={contextPane.mode === 'document' ? documentFocused : contextPaneFocused}
          onFocusToggle={() => {
            if (contextPane.mode === 'document') setDocumentFocused(value => !value);
            else setContextPaneFocused(value => !value);
          }}
        >
          {contextPane.mode === 'studio' && contextPane.workspaceRef && (
            <StudioTab
              isDark={isDark}
              workspaceRef={contextPane.workspaceRef}
              onDirtyChange={setStudioDirty}
            />
          )}
          {contextPane.mode === 'document' && contextPane.documentRef && (
            <Suspense fallback={<div className="context-pane-loading">正在加载文档编辑器…</div>}>
              <DocumentWorkspace
                documentRef={contextPane.documentRef}
                focused={documentFocused}
                onFocusChange={setDocumentFocused}
                onDirtyChange={setDocumentDirty}
                onAskAi={prompt => {
                  setDocumentFocused(false);
                  ai.setInput(prompt);
                  window.requestAnimationFrame(() => document.querySelector<HTMLTextAreaElement>('textarea')?.focus());
                }}
              />
            </Suspense>
          )}
          {contextPane.mode === 'collaboration' && contextPane.runtimeBatchId && (
            <CollaborationPanorama
              tasks={sessionRuntimeTasks}
              batchId={contextPane.runtimeBatchId}
              rootTitle={activeSession?.title || '协调协作者完成任务'}
              onOpenDocument={openPanoramaDocument}
              onOpenDiff={openPanoramaDiff}
            />
          )}
        </ContextPane>
      )}

      <DirectoryPickerModal
        open={directoryPicker.open}
        projects={folders}
        initialPath={directoryPicker.initialPath}
        onClose={() => setDirectoryPicker({ open: false, text: '' })}
        onConfirm={confirmNewSessionDirectory}
      />

    </div>
  );
}
