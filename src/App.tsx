import { useEffect, useState, useMemo, useRef, lazy, Suspense } from 'react';
import {
  Header,
  ToastContainer,
  useToast,
  showToast,
  useTheme,
  useWeather,
} from '@/shell';

import { useTodos } from '@/domain/todo';
import { DayPlanTimeline, useDayPlanReminders, useDayPlans } from '@/domain/day-plan';
import { usePomodoro, usePomodoroRecords } from '@/domain/pomodoro';
import {
  selectColdStartWorkbenchView,
  selectWorkItemProjection,
  useBackgroundTasks,
  useChatSessions,
  useTaskRuntime,
  type RuntimeNavigationRequest,
  type ChatMessage,
  type WorkItem,
  type WorkItemNavigationTarget,
} from '@/domain/chat';
import WorkbenchRightPanel, { type WorkbenchView } from '@/features/workbench/WorkbenchRightPanel';
import WorkStatusBar from '@/features/workbench/WorkStatusBar';
import {
  captureWorkItemSession,
  type WorkItemCaptureRequest,
} from '@/features/workbench/workItemCapture';
import { ToolboxCenter, useClipboardHistory } from '@/domain/toolbox';
import { SettingsModal, ModelUpdateDialog } from '@/features/settings';

import { usePeriodicSync } from '@/core/storage';
import { apiFetch } from '@/core/utils';
import { resolveInitialWorkspace } from '@/shell/state/workspaceNavigation';
import type { TodoItem, AppTab } from './types';

const ChatTab = lazy(() => import('@/domain/chat/ui/tab/ChatTab'));
const TodoTab = lazy(() => import('@/domain/todo/ui/tab/TodoTab'));

function TabLoading() {
  return (
    <div className="h-full flex items-center justify-center text-sm text-gray-500 dark:text-gray-400">
      正在加载...
    </div>
  );
}

export default function App() {
  return <SyncedWorkbenchApp />;
}

function SyncedWorkbenchApp() {
  usePeriodicSync();
  return <WorkbenchApp />;
}

function WorkbenchApp() {
  const [initialWorkspace] = useState(() => resolveInitialWorkspace(sessionStorage.getItem('pwb_active_tab')));
  const [activeTab, setActiveTab] = useState<AppTab>(initialWorkspace.activeTab);
  const [showSettings, setShowSettings] = useState(false);
  const [showToolbox, setShowToolbox] = useState(initialWorkspace.openToolbox);
  const [workbenchView, setWorkbenchView] = useState<WorkbenchView>('day_plan');
  const workbenchViewTouchedRef = useRef(false);
  const coldStartWorkItemsResolvedRef = useRef(false);
  const [requestedTodoId, setRequestedTodoId] = useState<string | null>(null);
  const [boundTodoId, setBoundTodoId] = useState<string | null>(null);
  const [runtimeNavigation, setRuntimeNavigation] = useState<RuntimeNavigationRequest>();
  const runtimeNavigationIdRef = useRef(0);

  useEffect(() => {
    sessionStorage.setItem('pwb_active_tab', activeTab);
  }, [activeTab]);

  const { todos, addTodo, cycleStatus, removeTodo, updateTodo, goals, addGoal, updateGoal, removeGoal } = useTodos();
  const dayPlans = useDayPlans();
  const { theme, toggleMode } = useTheme();
  const { weather, loading: weatherLoading, cities, selectedCity, setSelectedCity } = useWeather();
  const { records: pomodoroRecords, addRecord: addPomodoroRecord } = usePomodoroRecords();

  const boundTodoTitle = boundTodoId
    ? todos.find(t => t.id === boundTodoId)?.title
    : undefined;

  const pomodoro = usePomodoro(addPomodoroRecord, boundTodoId, boundTodoTitle);
  const chatSessions = useChatSessions();
  const importAgentSession = chatSessions.importAgentSession;
  const activateAgentSession = chatSessions.setActiveSessionId;
  const taskRuntime = useTaskRuntime();
  const daemonSessionsSyncedRef = useRef(false);
  const knownDaemonSessionIdsRef = useRef<Set<string>>(new Set());
  knownDaemonSessionIdsRef.current = new Set(chatSessions.sessions.flatMap(session => (
    session.agentSessionId ? [session.id, session.agentSessionId] : [session.id]
  )));
  const bgTasks = useBackgroundTasks();
  const { toasts, removeToast } = useToast();
  const clipboard = useClipboardHistory();
  const workItems = useMemo(
    () => selectWorkItemProjection(taskRuntime.tasks, chatSessions.sessions, taskRuntime.attention),
    [chatSessions.sessions, taskRuntime.attention, taskRuntime.tasks],
  );
  const sessionAttentionCounts = useMemo(() => {
    const counts: Record<string, number> = {};
    taskRuntime.attention
      .filter(item => item.status === 'unread')
      .forEach(item => {
        const session = chatSessions.sessions.find(candidate => (
          candidate.id === item.sessionId || candidate.agentSessionId === item.sessionId
        ));
        if (session) counts[session.id] = (counts[session.id] || 0) + 1;
      });
    return counts;
  }, [chatSessions.sessions, taskRuntime.attention]);

  useEffect(() => {
    if (daemonSessionsSyncedRef.current) return;
    daemonSessionsSyncedRef.current = true;
    let cancelled = false;
    const requestedSessionId = new URLSearchParams(window.location.search).get('agentSessionId');
    void (async () => {
      try {
        const sessions = await apiFetch<Array<[string, string]>>('/api/agent/sessions');
        for (const [sessionId] of sessions) {
          if (cancelled) return;
          if (knownDaemonSessionIdsRef.current.has(sessionId)) continue;
          const snapshot = await apiFetch<{
            id: string;
            name: string;
            messages: ChatMessage[];
            cwd?: string;
          }>(`/api/agent/sessions/${encodeURIComponent(sessionId)}/snapshot`);
          const localSessionId = importAgentSession(snapshot);
          knownDaemonSessionIdsRef.current.add(sessionId);
          if (requestedSessionId === sessionId) {
            activateAgentSession(localSessionId);
            setActiveTab('chat');
          }
        }
      } catch (error) {
        console.warn('[chat] Failed to import daemon-owned sessions:', error);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [activateAgentSession, importAgentSession]);

  useEffect(() => {
    if (!taskRuntime.loaded || coldStartWorkItemsResolvedRef.current) return;
    setWorkbenchView(current => selectColdStartWorkbenchView(
      workItems,
      current,
      workbenchViewTouchedRef.current,
    ));
    coldStartWorkItemsResolvedRef.current = true;
  }, [taskRuntime.loaded, workItems]);
  const requestNewChat = (
    text: string,
    options: { taskId?: string } = {},
    initialPath?: string,
  ) => {
    sessionStorage.setItem('pwb_pending_new_chat', JSON.stringify({ text, options, initialPath }));
    setActiveTab('chat');
  };

  // 今日累计专注分钟数（mode='work' 的会话；startTime 格式 yyyyMMdd HH:mm:ss）
  const pomodoroTodayMinutes = useMemo(() => {
    const t = new Date();
    const yyyyMMdd = `${t.getFullYear()}${String(t.getMonth() + 1).padStart(2, '0')}${String(t.getDate()).padStart(2, '0')}`;
    return Math.round(
      pomodoroRecords
        .filter(r => r.mode === 'work' && r.startTime.startsWith(yyyyMMdd))
        .reduce((sum, r) => sum + r.duration, 0) / 60,
    );
  }, [pomodoroRecords]);

  const normalTodos = todos;
  useDayPlanReminders(dayPlans.plans);

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
      const target = event.target as HTMLElement | null;
      const editable = Boolean(target?.closest('input, textarea, select, [contenteditable="true"], .monaco-editor'));
      if (event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setShowToolbox(true);
        return;
      }
      if (editable) return;
      if (event.key === '1') {
        event.preventDefault();
        setActiveTab('workbench');
      } else if (event.key === '2') {
        event.preventDefault();
        setActiveTab('chat');
      }
    };
    document.addEventListener('keydown', handleShortcut);
    return () => document.removeEventListener('keydown', handleShortcut);
  }, []);

  const openTodo = (todoId: string) => {
    setActiveTab('workbench');
    workbenchViewTouchedRef.current = true;
    setWorkbenchView('objectives');
    setRequestedTodoId(todoId);
  };

  const openWorkItem = (item: WorkItem, target: WorkItemNavigationTarget) => {
    const session = item.sessionId
      ? chatSessions.sessions.find(candidate => candidate.id === item.sessionId)
      : chatSessions.sessions.find(candidate => (
        candidate.agentSessionId === item.task.rootSessionId
        || candidate.id === item.task.rootSessionId
      ));
    if (!session) {
      showToast({ message: '关联会话已不存在，未将此事项标为已读', type: 'error' });
      return;
    }
    chatSessions.setActiveSessionId(session.id);
    runtimeNavigationIdRef.current += 1;
    setRuntimeNavigation({
      id: runtimeNavigationIdRef.current,
      taskId: item.task.id,
      sessionId: session.id,
      target,
    });
    setActiveTab('chat');
    item.attention
      .filter(attention => attention.status === 'unread')
      .forEach(attention => {
        void taskRuntime.markAttentionViewed(attention.id).catch(error => {
          console.warn('[runtime] Failed to mark attention viewed:', error);
        });
      });
  };

  const handleCaptureWorkItem = async (request: WorkItemCaptureRequest) => {
    const snapshot = await captureWorkItemSession(request);
    const localSessionId = chatSessions.importAgentSession(snapshot);
    chatSessions.setActiveSessionId(localSessionId);
    setActiveTab('chat');
  };

  const handleBindTodo = (todoId: string | null) => {
    setBoundTodoId(todoId);
    if (todoId) {
      const title = todos.find(t => t.id === todoId)?.title;
      showToast({ message: `已绑定任务：${title}`, type: 'info' });
    } else {
      showToast({ message: '已取消任务绑定', type: 'info' });
    }
  };

  const handleStartPomodoroForTodo = (todo: TodoItem) => {
    const direction = todo.focusTimerMode || 'countdown';
    const workSeconds = direction === 'countdown'
      ? Math.max(5, Math.min(120, todo.focusMinutes || todo.estimatedMinutes || 25)) * 60
      : undefined;
    pomodoro.startWorkSession({ direction, workSeconds });
  };


  return (
    <div className={`app-root ${theme.mode}`}>
      <div className="app-container">
        <Header
          activeTab={activeTab}
          onTabChange={setActiveTab}
          theme={theme}
          onToggleMode={toggleMode}
          onOpenSettings={() => setShowSettings(true)}
          onOpenToolbox={() => setShowToolbox(true)}
        />

        <main className={`main-content main-content-${activeTab} ${activeTab === 'workbench' ? `main-content-workbench-${workbenchView}` : ''} relative`}>
            {activeTab === 'chat' ? (
              <div className="h-full min-w-0">
                <Suspense fallback={<TabLoading />}>
                  <ChatTab
                    sessions={chatSessions.sessions}
                    activeSessionId={chatSessions.activeSessionId}
                    activeSession={chatSessions.activeSession}
                    onSelectSession={chatSessions.setActiveSessionId}
                    onCreateSession={(model, options) => chatSessions.createSession(model, options)}
                    onDeleteSession={chatSessions.deleteSession}
                    onUpdateSessionMessages={chatSessions.updateSessionMessages}
                    onUpdateSession={chatSessions.updateSession}
                    folders={chatSessions.folders}
                    onCreateFolder={chatSessions.createFolder}
                    onRenameFolder={chatSessions.renameFolder}
                    onSetFolderPinned={chatSessions.setFolderPinned}
                    onDeleteFolder={chatSessions.deleteFolder}
                    onMoveSessionToFolder={chatSessions.moveSessionToFolder}
                    onReorderSessions={chatSessions.reorderSessions}
                    onReorderFolders={chatSessions.reorderFolders}
                    bgTasks={bgTasks}
                    todos={normalTodos}
                    goals={goals}
                    onUpdateTodo={updateTodo}
                    onOpenTodo={openTodo}
                    isDark={theme.mode === 'dark'}
                    runtimeTasks={taskRuntime.tasks}
                    attentionCounts={sessionAttentionCounts}
                    onCancelRuntimeTask={taskRuntime.cancel}
                    onRetryRuntimeTask={taskRuntime.retry}
                    onResolveRuntimeTaskDecision={taskRuntime.resolveDecision}
                    onFollowUpRuntimeTask={taskRuntime.followUp}
                    onReviewRuntimeTask={taskRuntime.review}
                    runtimeNavigation={runtimeNavigation}
                    onRuntimeNavigationHandled={id => {
                      setRuntimeNavigation(current => current?.id === id ? undefined : current);
                    }}
                  />
                </Suspense>
              </div>
            ) : (
              <div className="content-grid">
                <div className="main-area">
                  <WorkbenchRightPanel
                    activeView={workbenchView}
                    onActiveViewChange={view => {
                      workbenchViewTouchedRef.current = true;
                      setWorkbenchView(view);
                    }}
                    todos={normalTodos}
                    dayPlans={dayPlans.plans}
                    onAskAiForJob={() => {
                      requestNewChat('请帮我创建一个定时调度任务。先询问我执行目标、时间、工作目录和可接受的副作用，再生成配置并真实测试；只有测试通过后才正式启用。');
                    }}
                    workItems={workItems}
                    onOpenWorkItem={openWorkItem}
                    onResolveAttention={taskRuntime.resolveAttention}
                    onDeleteAttention={taskRuntime.deleteAttention}
                    onRetryWorkItem={async (item) => {
                      await taskRuntime.retry(item.task.id);
                    }}
                    projects={chatSessions.folders}
                    onCaptureWorkItem={handleCaptureWorkItem}
                    objectivePanel={
                      <Suspense fallback={<TabLoading />}>
                        <TodoTab
                          todos={normalTodos}
                          onAdd={(title, type, notesText, priority, dueDate, subTasks, goalId, keyResultId) =>
                            addTodo(title, type, notesText, priority, dueDate, subTasks, goalId, keyResultId)
                          }
                          onCycleStatus={cycleStatus}
                          onRemove={removeTodo}
                          onUpdate={updateTodo}
                          requestedTodoId={requestedTodoId}
                          onRequestedTodoHandled={() => setRequestedTodoId(null)}
                          pomodoroIsRunning={pomodoro.isRunning}
                          pomodoroTodayMinutes={pomodoroTodayMinutes}
                          boundTodoId={boundTodoId}
                          onBindTodo={handleBindTodo}
                          onStartPomodoro={handleStartPomodoroForTodo}
                          goals={goals}
                          onAddGoal={addGoal}
                          onUpdateGoal={updateGoal}
                          onRemoveGoal={removeGoal}
                          onStartAi={todo => {
                            requestNewChat(todo.title, {
                              taskId: todo.id,
                            }, todo.executionWorkspace?.path);
                          }}
                        />
                      </Suspense>
                    }
                    dayPlanPanel={
                      <DayPlanTimeline
                        plans={dayPlans.plans}
                        todos={normalTodos}
                        onAdd={dayPlans.addPlan}
                        onUpdate={dayPlans.updatePlan}
                        onRemove={dayPlans.removePlan}
                        onRestore={dayPlans.restorePlan}
                      />
                    }
                  />
                </div>
              </div>
            )}
        </main>

        <WorkStatusBar
          taskTitle={boundTodoTitle ?? null}
          boundTaskId={boundTodoId}
          tasks={todos.filter(todo => !todo.completed).map(todo => ({ id: todo.id, title: todo.title }))}
          onBindTask={handleBindTodo}
          onOpenTasks={() => setActiveTab('workbench')}
          timerActive={
            pomodoro.isRunning ||
            (pomodoro.timerDirection === 'countup'
              ? pomodoro.elapsedSeconds > 0
              : pomodoro.timeLeft < pomodoro.totalSeconds)
          }
          mode={pomodoro.mode}
          timerDirection={pomodoro.timerDirection}
          timeLeft={pomodoro.displaySeconds}
          isRunning={pomodoro.isRunning}
          timerSettings={pomodoro.timerSettings}
          onStart={() => {
            const boundTodo = boundTodoId ? todos.find(todo => todo.id === boundTodoId) : undefined;
            if (boundTodo) {
              handleStartPomodoroForTodo(boundTodo);
            } else {
              pomodoro.startWorkSession({ direction: 'countdown' });
            }
          }}
          onToggle={pomodoro.toggle}
          onSkip={pomodoro.skip}
          onReset={pomodoro.reset}
          onUpdateSettings={pomodoro.updateTimerSettings}
          formatTime={pomodoro.formatTime}
          backgroundTasks={bgTasks}
          weather={weather}
          weatherLoading={weatherLoading}
          cities={cities}
          selectedCity={selectedCity}
          onCityChange={setSelectedCity}
        />
      </div>

      <SettingsModal open={showSettings} onClose={() => setShowSettings(false)} />
      <ModelUpdateDialog onOpenSettings={() => setShowSettings(true)} />
      <ToolboxCenter
        open={showToolbox}
        onClose={() => setShowToolbox(false)}
        history={clipboard.history}
        onAdd={clipboard.addItem}
        onRemove={clipboard.removeItem}
        onClear={clipboard.clear}
        onCopy={clipboard.copyToClipboard}
      />
      <ToastContainer toasts={toasts} onRemove={removeToast} />
    </div>
  );
}
