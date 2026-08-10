import { useState, useCallback, useRef, useMemo } from 'react';
import { load, save, KEYS, useStorageSync } from '@/core/storage';
import { generateId } from '@/core/utils/id';
import { now } from '@/core/utils/date';
import type { ChatSession, ChatMessage, AiModel, LocalProject } from '../types';
import { getModels, modelSelectionKey } from '@/core/config';
import { apiFetch } from '@/core/utils';

// --- Throttled persistence for streaming (module-level singleton) ---
const THROTTLE_MS = 500;
let _throttleTimer: ReturnType<typeof setTimeout> | null = null;
let _pendingSessions: ChatSession[] | null = null;

/**
 * Throttled version of save(KEYS.CHAT_SESSIONS, ...).
 * During streaming, call this instead of immediate save to avoid
 * JSON.stringify-ing 3MB of sessions hundreds of times per second.
 */
export function throttledPersistSessions(next: ChatSession[]): void {
  _pendingSessions = next;
  if (_throttleTimer === null) {
    _throttleTimer = setTimeout(() => {
      _throttleTimer = null;
      if (_pendingSessions !== null) {
        save(KEYS.CHAT_SESSIONS, _pendingSessions);
        _pendingSessions = null;
      }
    }, THROTTLE_MS);
  }
}

/**
 * Immediately flush any pending throttled persistence.
 * Call at stream end to ensure final state is persisted.
 */
export function flushThrottledPersist(): void {
  if (_throttleTimer !== null) {
    clearTimeout(_throttleTimer);
    _throttleTimer = null;
  }
  if (_pendingSessions !== null) {
    save(KEYS.CHAT_SESSIONS, _pendingSessions);
    _pendingSessions = null;
  }
}

function makeTitleFromMessages(msgs: ChatMessage[]): string {
  const firstUser = msgs.find(m => m.role === 'user');
  if (firstUser?.content) {
    const text = firstUser.content.trim().slice(0, 20);
    return text.length >= 20 ? text + '…' : text;
  }
  return '新对话';
}

/** Strip messages from sessions to reduce memory — sidebar only needs metadata. */
function stripMessages(sessions: ChatSession[]): ChatSession[] {
  return sessions.map(s => s.messages.length === 0 ? s : { ...s, messages: [] });
}

export function normalizeChatSession(session: ChatSession): ChatSession {
  const normalized: ChatSession = {
    ...session,
    mode: 'agent',
    // Supervisor history has been imported into RuntimeTask. Keeping these
    // flags enabled would reconnect the removed polling/control plane.
    collaboration: undefined,
    supervisorTaskId: undefined,
  };
  return {
    ...normalized,
    projectId: normalized.projectId ?? (normalized.cwd ? projectIdForPath(normalized.cwd) : undefined),
    workspaceStatus: normalized.workspaceStatus ?? (normalized.cwd ? 'ready' : 'awaiting_binding'),
  };
}

function normalizeSessions(sessions: ChatSession[]): ChatSession[] {
  return sessions.map(normalizeChatSession);
}

/** Read messages for a single session from localStorage (on demand). */
function loadMessagesForSession(id: string): ChatMessage[] {
  const all = normalizeSessions(load<ChatSession[]>(KEYS.CHAT_SESSIONS, []));
  return all.find(s => s.id === id)?.messages || [];
}

/**
 * Read full sessions from localStorage (with messages).
 * Used internally for persistence — callers that only need metadata use the
 * stripped React state instead.
 */
function loadFullSessions(): ChatSession[] {
  return normalizeSessions(load<ChatSession[]>(KEYS.CHAT_SESSIONS, []));
}

function projectIdForPath(path: string): string {
  let hash = 2166136261;
  for (const char of path) {
    hash ^= char.charCodeAt(0);
    hash = Math.imul(hash, 16777619);
  }
  return `project_${(hash >>> 0).toString(36)}`;
}

function projectName(path: string): string {
  const parts = path.replace(/\/$/, '').split('/').filter(Boolean);
  return parts[parts.length - 1] || path;
}

function normalizeProjects(raw: LocalProject[], sessions: ChatSession[]): LocalProject[] {
  const byPath = new Map<string, LocalProject>();
  raw.forEach(project => {
    if (!project.canonicalPath) return;
    byPath.set(project.canonicalPath, {
      ...project,
      id: project.id || projectIdForPath(project.canonicalPath),
      displayPath: project.displayPath || project.canonicalPath,
      availability: project.availability || 'ready',
      lastOpenedAt: project.lastOpenedAt || project.createdAt || now(),
    });
  });
  sessions.forEach(session => {
    if (!session.cwd || byPath.has(session.cwd)) return;
    const ts = session.updatedAt || session.createdAt || now();
    byPath.set(session.cwd, {
      id: projectIdForPath(session.cwd),
      name: projectName(session.cwd),
      canonicalPath: session.cwd,
      displayPath: session.cwdDisplayPath || session.cwd,
      availability: session.workspaceStatus === 'missing' ? 'missing' : session.workspaceStatus === 'denied' ? 'denied' : 'ready',
      createdAt: session.createdAt || ts,
      lastOpenedAt: ts,
    });
  });
  return [...byPath.values()].sort((left, right) => right.lastOpenedAt.localeCompare(left.lastOpenedAt));
}

export function useChatSessions() {
  // React state holds sessions WITHOUT messages (sidebar metadata only).
  const [sessions, setSessions] = useState<ChatSession[]>(() =>
    stripMessages(normalizeSessions(load(KEYS.CHAT_SESSIONS, [])))
  );
  const [folders, setFolders] = useState<LocalProject[]>(() => {
    const normalizedSessions = normalizeSessions(load(KEYS.CHAT_SESSIONS, []));
    const stored = load<LocalProject[]>(KEYS.CHAT_PROJECTS, []);
    return normalizeProjects(stored, normalizedSessions);
  });
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);

  useStorageSync(() => {
    setSessions(stripMessages(normalizeSessions(load(KEYS.CHAT_SESSIONS, []))));
    const normalizedSessions = normalizeSessions(load(KEYS.CHAT_SESSIONS, []));
    setFolders(normalizeProjects(load(KEYS.CHAT_PROJECTS, []), normalizedSessions));
  });

  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  const foldersRef = useRef(folders);
  foldersRef.current = folders;

  /**
   * Persist full sessions array to storage. `metaSessions` is the stripped
   * array (for React state); `fullPatch` is a map of id→messages for sessions
   * whose messages changed. Non-patched sessions read their messages from
   * localStorage to keep them intact.
   */
  const persistSessionsRef = useRef((metaSessions: ChatSession[], messagesPatch?: Map<string, ChatMessage[]>) => {
    // React state updates are asynchronous. Keep same-tick writers on the
    // latest metadata so the first streamed message cannot overwrite a newly
    // assigned agentSessionId with the previous snapshot.
    sessionsRef.current = metaSessions;
    setSessions(metaSessions);
    if (messagesPatch && messagesPatch.size > 0) {
      const existing = loadFullSessions();
      const existingMap = new Map(existing.map(s => [s.id, s.messages]));
      const full = metaSessions.map(s => {
        const patched = messagesPatch.get(s.id);
        if (patched !== undefined) return { ...s, messages: patched };
        return { ...s, messages: existingMap.get(s.id) || s.messages };
      });
      save(KEYS.CHAT_SESSIONS, full);
    } else {
      const existing = loadFullSessions();
      const existingMap = new Map(existing.map(s => [s.id, s.messages]));
      const full = metaSessions.map(s => ({
        ...s,
        messages: existingMap.get(s.id) || s.messages,
      }));
      save(KEYS.CHAT_SESSIONS, full);
    }
  });
  const persistSessions = persistSessionsRef.current;

  const persistFoldersRef = useRef((next: LocalProject[]) => {
    setFolders(next);
    save(KEYS.CHAT_PROJECTS, next);
  });
  const persistFolders = persistFoldersRef.current;

  const createSession = useCallback((
    model?: AiModel,
    options: Partial<Pick<ChatSession,
      'mode' | 'cwd' | 'taskId'
    >> = {},
  ) => {
    const id = generateId();
    const ts = now();
    const project = options.cwd
      ? foldersRef.current.find(item => item.canonicalPath === options.cwd) ?? {
          id: projectIdForPath(options.cwd),
          name: projectName(options.cwd),
          canonicalPath: options.cwd,
          displayPath: options.cwd,
          availability: 'ready' as const,
          createdAt: ts,
          lastOpenedAt: ts,
        }
      : undefined;
    if (project && !foldersRef.current.some(item => item.id === project.id)) {
      persistFolders([project, ...foldersRef.current]);
    }
    const session: ChatSession = {
      id,
      title: '新对话',
      messages: [],
      model: model || (getModels()[0] ? modelSelectionKey(getModels()[0]) : ''),
      ...options,
      projectId: project?.id,
      workspaceStatus: options.cwd ? 'ready' : 'awaiting_binding',
      createdAt: ts,
      updatedAt: ts,
    };
    persistSessions([session, ...sessionsRef.current]);
    setActiveSessionId(id);
    return session;
  }, [persistFolders, persistSessions]);

  const deleteSession = useCallback((id: string) => {
    const next = sessionsRef.current.filter(s => s.id !== id);
    // Persist stripped metadata; localStorage messages for deleted session get dropped naturally
    const full = loadFullSessions().filter(s => s.id !== id);
    setSessions(next);
    save(KEYS.CHAT_SESSIONS, full);
    void apiFetch(`/api/image-artifacts/sessions/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }).catch(error => {
      console.warn('[chat] Failed to clean image artifacts for deleted session:', error);
    });
    setActiveSessionId(prev => {
      if (prev === id) {
        return next.length ? next[0].id : null;
      }
      return prev;
    });
  }, []);

  const updateSession = useCallback((id: string, patch: Partial<ChatSession>) => {
    const { messages: _msgs, ...metaPatch } = patch;
    const next = sessionsRef.current.map(s =>
      s.id === id ? { ...s, ...metaPatch, updatedAt: now() } : s
    );
    const messagesPatch = _msgs !== undefined ? new Map([[id, _msgs]]) : undefined;
    persistSessions(next, messagesPatch);
  }, [persistSessions]);

  const updateSessionMessages = useCallback((id: string, messages: ChatMessage[]) => {
    const next = sessionsRef.current.map(s => {
      if (s.id !== id) return s;
      const title = s.title === '新对话' && messages.some(m => m.role === 'user')
        ? makeTitleFromMessages(messages)
        : s.title;
      return { ...s, title, updatedAt: now() };
    });
    persistSessions(next, new Map([[id, messages]]));
  }, [persistSessions]);

  const createFolder = useCallback((canonicalPath: string) => {
    const existing = foldersRef.current.find(project => project.canonicalPath === canonicalPath);
    if (existing) return existing;
    const ts = now();
    const project: LocalProject = {
      id: projectIdForPath(canonicalPath),
      name: projectName(canonicalPath),
      canonicalPath,
      displayPath: canonicalPath,
      availability: 'ready',
      createdAt: ts,
      lastOpenedAt: ts,
    };
    persistFolders([project, ...foldersRef.current]);
    return project;
  }, [persistFolders]);

  const renameFolder = useCallback((id: string, name: string) => {
    const next = foldersRef.current.map(f =>
      f.id === id ? { ...f, name: name.trim() || f.name } : f
    );
    persistFolders(next);
  }, [persistFolders]);

  const setFolderPinned = useCallback((id: string, pinned: boolean) => {
    const next = foldersRef.current.map(folder =>
      folder.id === id ? { ...folder, pinned } : folder
    );
    persistFolders(next);
  }, [persistFolders]);

  const deleteFolder = useCallback((id: string) => {
    if (sessionsRef.current.some(session => session.projectId === id)) return;
    const nextFolders = foldersRef.current.filter(f => f.id !== id);
    persistFolders(nextFolders);
  }, [persistFolders]);

  const moveSessionToFolder = useCallback((sessionId: string, folderId: string | null) => {
    const session = sessionsRef.current.find(item => item.id === sessionId);
    if (!session || session.projectId === folderId) return;
    console.warn('[chat] Bound conversations cannot be moved between local projects.');
  }, []);

  const reorderSessions = useCallback((orderedIds: string[]) => {
    const existingMap = new Map(sessionsRef.current.map(s => [s.id, s]));
    const seen = new Set<string>();
    const next: ChatSession[] = [];

    for (const id of orderedIds) {
      const s = existingMap.get(id);
      if (s && !seen.has(s.id)) {
        next.push(s);
        seen.add(s.id);
      }
    }

    for (const s of sessionsRef.current) {
      if (!seen.has(s.id)) {
        next.push(s);
        seen.add(s.id);
      }
    }

    persistSessions(next);
  }, [persistSessions]);

  const reorderFolders = useCallback((orderedIds: string[]) => {
    const existingMap = new Map(foldersRef.current.map(f => [f.id, f]));
    const next: LocalProject[] = [];
    for (const id of orderedIds) {
      const f = existingMap.get(id);
      if (f) next.push(f);
    }
    for (const f of foldersRef.current) {
      if (!orderedIds.includes(f.id)) next.push(f);
    }
    persistFolders(next);
  }, [persistFolders]);

  const importAgentSession = useCallback((snapshot: {
    id: string;
    name: string;
    messages: ChatMessage[];
    cwd?: string;
  }) => {
    const existing = sessionsRef.current.find(session => (
      session.agentSessionId === snapshot.id || session.id === snapshot.id
    ));
    if (existing) {
      updateSession(existing.id, {
        title: snapshot.name || existing.title,
        cwd: snapshot.cwd || existing.cwd,
        workspaceStatus: snapshot.cwd ? 'ready' : existing.workspaceStatus,
        agentSessionId: snapshot.id,
        messages: snapshot.messages,
      });
      return existing.id;
    }
    const ts = now();
    const project = snapshot.cwd
      ? foldersRef.current.find(item => item.canonicalPath === snapshot.cwd) ?? {
          id: projectIdForPath(snapshot.cwd),
          name: projectName(snapshot.cwd),
          canonicalPath: snapshot.cwd,
          displayPath: snapshot.cwd,
          availability: 'ready' as const,
          createdAt: ts,
          lastOpenedAt: ts,
        }
      : undefined;
    if (project && !foldersRef.current.some(item => item.id === project.id)) {
      persistFolders([project, ...foldersRef.current]);
    }
    const session: ChatSession = {
      id: snapshot.id,
      title: snapshot.name || '后台会话',
      messages: snapshot.messages,
      model: getModels()[0] ? modelSelectionKey(getModels()[0]) : '',
      projectId: project?.id,
      cwd: snapshot.cwd,
      workspaceStatus: snapshot.cwd ? 'ready' : 'awaiting_binding',
      agentSessionId: snapshot.id,
      createdAt: ts,
      updatedAt: ts,
    };
    persistSessions([session, ...sessionsRef.current], new Map([[session.id, snapshot.messages]]));
    return session.id;
  }, [persistFolders, persistSessions, updateSession]);

  // activeSession: metadata from React state + messages loaded on demand
  const activeSession = useMemo(() => {
    const meta = sessions.find(s => s.id === activeSessionId);
    if (!meta) return null;
    const messages = loadMessagesForSession(meta.id);
    return { ...meta, messages };
  }, [sessions, activeSessionId]);

  return {
    sessions,
    folders,
    activeSessionId,
    activeSession,
    setActiveSessionId,
    createSession,
    deleteSession,
    updateSession,
    updateSessionMessages,
    createFolder,
    renameFolder,
    setFolderPinned,
    deleteFolder,
    moveSessionToFolder,
    reorderSessions,
    reorderFolders,
    importAgentSession,
  };
}
