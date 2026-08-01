import { useMemo, useState } from 'react';
import { Check, ChevronRight, Edit3, Folder, MoreHorizontal, PanelLeftClose, Plus, Trash2 } from 'lucide-react';
import type { ChatSession, LocalProject, SessionRuntime } from '@/domain/chat';

export interface ChatSidebarProps {
  sessions: ChatSession[];
  projects: LocalProject[];
  activeSessionId: string | null;
  onSelectSession: (id: string) => void;
  onCreateSession: (initialPath?: string) => void;
  onDeleteSession: (id: string) => void;
  onUpdateSession: (id: string, patch: Partial<ChatSession>) => void;
  onRemoveProject?: (id: string) => void;
  collapsed?: boolean;
  onToggleCollapse?: () => void;
  attentionCounts?: Record<string, number>;
  runtimes?: Record<string, SessionRuntime>;
  delegationCounts?: Record<string, number>;
}

export default function ChatSidebar({
  sessions,
  projects,
  activeSessionId,
  onSelectSession,
  onCreateSession,
  onDeleteSession,
  onUpdateSession,
  onRemoveProject,
  collapsed = false,
  onToggleCollapse,
  attentionCounts = {},
  runtimes = {},
  delegationCounts = {},
}: ChatSidebarProps) {
  const [collapsedProjects, setCollapsedProjects] = useState<Set<string>>(() => new Set());
  const [editingSessionId, setEditingSessionId] = useState<string | null>(null);
  const [editingTitle, setEditingTitle] = useState('');
  const [openProjectMenu, setOpenProjectMenu] = useState<string | null>(null);

  const sessionsByProject = useMemo(() => {
    const map = new Map<string, ChatSession[]>();
    projects.forEach(project => map.set(project.id, []));
    sessions.forEach(session => {
      if (session.projectId && map.has(session.projectId)) map.get(session.projectId)!.push(session);
    });
    map.forEach(items => items.sort((left, right) => right.updatedAt.localeCompare(left.updatedAt)));
    return map;
  }, [projects, sessions]);
  const unbound = useMemo(
    () => sessions.filter(session => !session.projectId || !projects.some(project => project.id === session.projectId)),
    [projects, sessions],
  );

  const moveFocus = (sessionId: string, direction: -1 | 1 | 'first' | 'last') => {
    const buttons = Array.from(document.querySelectorAll<HTMLButtonElement>('[data-session-primary="true"]'));
    const current = buttons.findIndex(button => button.dataset.sessionId === sessionId);
    const target = direction === 'first'
      ? buttons[0]
      : direction === 'last'
        ? buttons[buttons.length - 1]
        : buttons[current + direction];
    target?.focus();
  };

  const renderSession = (session: ChatSession) => {
    const runtime = session.agentSessionId ? runtimes[session.agentSessionId] : undefined;
    const queueCount = runtime?.queue.length ?? 0;
    const running = runtime?.phase === 'running';
    const attention = attentionCounts[session.id] || 0;
    const delegations = delegationCounts[session.id] || 0;
    const isEditing = editingSessionId === session.id;
    return (
      <div key={session.id} className={`pwb-session group pwb-session-nested ${activeSessionId === session.id ? 'pwb-session-active' : ''}`} role="listitem">
        {isEditing ? (
          <div className="flex w-full items-center gap-1.5">
            <input
              value={editingTitle}
              onChange={event => setEditingTitle(event.target.value)}
              onKeyDown={event => {
                if (event.key === 'Enter') {
                  onUpdateSession(session.id, { title: editingTitle.trim() || session.title });
                  setEditingSessionId(null);
                } else if (event.key === 'Escape') setEditingSessionId(null);
              }}
              className="precision-inline-editor min-w-0 flex-1"
              autoFocus
            />
            <button type="button" className="precision-sidebar-icon-button" aria-label="确认重命名" onClick={() => {
              onUpdateSession(session.id, { title: editingTitle.trim() || session.title });
              setEditingSessionId(null);
            }}><Check size={13} /></button>
          </div>
        ) : (
          <>
            <button
              type="button"
              data-session-primary="true"
              data-session-id={session.id}
              aria-current={activeSessionId === session.id ? 'page' : undefined}
              className="pwb-session-primary min-w-0 flex-1 text-left"
              onClick={() => onSelectSession(session.id)}
              onKeyDown={event => {
                if (event.key === 'ArrowDown') { event.preventDefault(); moveFocus(session.id, 1); }
                else if (event.key === 'ArrowUp') { event.preventDefault(); moveFocus(session.id, -1); }
                else if (event.key === 'Home') { event.preventDefault(); moveFocus(session.id, 'first'); }
                else if (event.key === 'End') { event.preventDefault(); moveFocus(session.id, 'last'); }
              }}
            >
              <span className="truncate">{session.title}</span>
              <span className="pwb-session-badges" aria-label="会话状态">
                {running && <i className="is-running" title="运行中" />}
                {queueCount > 0 && <small title={`${queueCount} 条排队消息`}>{queueCount}</small>}
                {delegations > 0 && <small title={`${delegations} 个委派任务`}>↗{delegations}</small>}
                {attention > 0 && <small className="is-attention" title={`${attention} 个待处理事项`}>{attention}</small>}
              </span>
            </button>
            <div className="pwb-session-actions">
              <button type="button" className="precision-sidebar-icon-button" aria-label={`重命名会话：${session.title}`} onClick={() => {
                setEditingSessionId(session.id);
                setEditingTitle(session.title);
              }}><Edit3 size={12} /></button>
              <button type="button" className="precision-sidebar-icon-button precision-danger-button" aria-label={`删除会话：${session.title}`} onClick={() => onDeleteSession(session.id)}><Trash2 size={12} /></button>
            </div>
          </>
        )}
      </div>
    );
  };

  return (
    <aside className={`pwb-sidebar ${collapsed ? 'collapsed' : ''}`}>
      <div className="flex h-full flex-col">
        <header className="pwb-sidebar-header">
          <div><small>AI 工作区</small><h2>本地项目</h2></div>
          <div>
            <button type="button" className="pwb-sidebar-action" title="新建会话" onClick={() => onCreateSession()}><Plus size={15} /></button>
            {onToggleCollapse && <button type="button" className="pwb-sidebar-action" title="收起侧栏" onClick={onToggleCollapse}><PanelLeftClose size={15} /></button>}
          </div>
        </header>

        <div className="pwb-project-list" role="list" aria-label="本地项目和会话">
          {projects.map(project => {
            const projectSessions = sessionsByProject.get(project.id) ?? [];
            const isCollapsed = collapsedProjects.has(project.id);
            const menuOpen = openProjectMenu === project.id;
            return (
              <section key={project.id} className="pwb-project" role="listitem">
                <div className="pwb-project-row group">
                  <button type="button" className="pwb-project-primary" aria-expanded={!isCollapsed} onClick={() => {
                    setCollapsedProjects(current => {
                      const next = new Set(current);
                      if (next.has(project.id)) next.delete(project.id); else next.add(project.id);
                      return next;
                    });
                  }}>
                    <Folder size={16} />
                    <span title={project.displayPath}>{project.name}</span>
                    {project.availability !== 'ready' && <small>{project.availability === 'missing' ? '目录丢失' : '无权限'}</small>}
                    <ChevronRight size={13} className={isCollapsed ? '' : 'rotate-90'} />
                  </button>
                  <div className="pwb-project-actions">
                    <button type="button" aria-label={`在 ${project.name} 新建会话`} onClick={() => onCreateSession(project.canonicalPath)}><Plus size={14} /></button>
                    <button type="button" aria-label={`${project.name} 更多操作`} onClick={() => setOpenProjectMenu(menuOpen ? null : project.id)}><MoreHorizontal size={15} /></button>
                  </div>
                  {menuOpen && (
                    <div className="precision-popover pwb-project-menu">
                      <button type="button" disabled={projectSessions.length > 0} onClick={() => {
                        onRemoveProject?.(project.id);
                        setOpenProjectMenu(null);
                      }}>移除项目</button>
                      {projectSessions.length > 0 && <small>删除项目下的会话后才能移除</small>}
                    </div>
                  )}
                </div>
                {!isCollapsed && (
                  <div className="pwb-project-sessions" role="list" aria-label={`${project.name} 会话`}>
                    {projectSessions.length > 0
                      ? projectSessions.map(renderSession)
                      : <div className="pwb-project-empty">无会话</div>}
                  </div>
                )}
              </section>
            );
          })}

          {unbound.length > 0 && (
            <section className="pwb-project is-unbound" role="listitem">
              <div className="pwb-project-row"><div className="pwb-project-primary"><Folder size={16} /><span>待绑定历史</span><small>{unbound.length}</small></div></div>
              <div className="pwb-project-sessions" role="list">{unbound.map(renderSession)}</div>
            </section>
          )}

          {projects.length === 0 && unbound.length === 0 && (
            <div className="pwb-projects-empty"><Folder size={22} /><strong>还没有本地项目</strong><span>新建会话时选择一个工作文件夹</span><button type="button" onClick={() => onCreateSession()}>选择文件夹</button></div>
          )}
        </div>
      </div>
    </aside>
  );
}
