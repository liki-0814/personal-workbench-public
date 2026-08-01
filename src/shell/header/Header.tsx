import { useEffect, useRef, useState } from 'react';
import {
  ChevronLeft,
  ChevronRight,
  LayoutDashboard,
  Menu,
  MessageSquare,
  Moon,
  Settings,
  Sun,
  Wrench,
  X,
  type LucideIcon,
} from 'lucide-react';
import type { AppTab } from '@/types/app';
import type { ThemeConfig } from '../types';
import { apiFetch } from '@/core/utils';

interface DaemonStatus {
  healthy: boolean;
  version?: string;
  activeTasks: number;
  activeAcpSessions: number;
}

const TABS: { id: AppTab; icon: LucideIcon; label: string; description: string }[] = [
  { id: 'workbench', icon: LayoutDashboard, label: '工作台', description: '目标、任务与今日安排' },
  { id: 'chat', icon: MessageSquare, label: 'AI', description: '提问、执行与验收' },
];

const RAIL_KEY = 'pwb_nav_collapsed';

interface Props {
  activeTab: AppTab;
  onTabChange: (tab: AppTab) => void;
  theme: ThemeConfig;
  onToggleMode: () => void;
  children?: React.ReactNode;
  onOpenSettings?: () => void;
  onOpenToolbox: () => void;
}

export default function Header({
  activeTab,
  onTabChange,
  theme,
  onToggleMode,
  children,
  onOpenSettings,
  onOpenToolbox,
}: Props) {
  const [collapsed, setCollapsed] = useState(() => sessionStorage.getItem(RAIL_KEY) !== '0');
  const [mobileMoreOpen, setMobileMoreOpen] = useState(false);
  const [daemonStatus, setDaemonStatus] = useState<DaemonStatus | null>(null);
  const mobileMoreRef = useRef<HTMLDivElement>(null);
  const suppressMobileMoreRestoreRef = useRef(false);
  const active = TABS.find(tab => tab.id === activeTab) ?? TABS[0];
  const ActiveIcon = active.icon;

  useEffect(() => {
    sessionStorage.setItem(RAIL_KEY, collapsed ? '1' : '0');
  }, [collapsed]);

  useEffect(() => {
    let alive = true;
    const refresh = () => apiFetch<DaemonStatus>('/api/agent/daemon/status')
      .then(status => { if (alive) setDaemonStatus(status); })
      .catch(() => { if (alive) setDaemonStatus(null); });
    void refresh();
    const timer = window.setInterval(refresh, 15_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    if (!mobileMoreOpen) {
      suppressMobileMoreRestoreRef.current = false;
      return;
    }
    const previousFocus = document.activeElement as HTMLElement | null;
    const focusTimer = window.setTimeout(() => {
      mobileMoreRef.current?.querySelector<HTMLElement>('button')?.focus();
    }, 0);
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        setMobileMoreOpen(false);
        return;
      }
      if (event.key !== 'Tab' || !mobileMoreRef.current) return;
      const focusable = Array.from(
        mobileMoreRef.current.querySelectorAll<HTMLElement>('button:not(:disabled)'),
      );
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.clearTimeout(focusTimer);
      document.removeEventListener('keydown', handleKeyDown);
      if (!suppressMobileMoreRestoreRef.current) previousFocus?.focus();
    };
  }, [mobileMoreOpen]);

  const selectTab = (tab: AppTab) => {
    onTabChange(tab);
    setMobileMoreOpen(false);
  };

  return (
    <>
      <aside className={`primary-rail ${collapsed ? 'is-collapsed' : ''}`} aria-label="主导航">
        <div className="rail-brand" title="Personal Workbench">
          <span className="rail-mark">PW</span>
          <span className="rail-brand-copy">
            <strong>Workbench</strong>
            <small>Personal desk</small>
          </span>
        </div>

        <nav className="rail-nav">
          {TABS.map(({ id, icon: Icon, label }) => (
            <button
              key={id}
              type="button"
              onClick={() => selectTab(id)}
              className={`rail-nav-item ${activeTab === id ? 'is-active' : ''}`}
              aria-current={activeTab === id ? 'page' : undefined}
              title={collapsed ? label : undefined}
            >
              <Icon size={17} strokeWidth={1.7} />
              <span>{label}</span>
            </button>
          ))}
        </nav>

        <div className="rail-actions">
          <button type="button" className="rail-nav-item" onClick={onOpenToolbox} title="打开工具中心 (⌘K)">
            <Wrench size={17} strokeWidth={1.7} />
            <span>工具中心</span>
          </button>
          {onOpenSettings && (
            <button type="button" className="rail-nav-item" onClick={onOpenSettings} title="设置">
              <Settings size={17} strokeWidth={1.7} />
              <span>设置</span>
            </button>
          )}
          <button
            type="button"
            className="rail-nav-item"
            onClick={onToggleMode}
            title={theme.mode === 'dark' ? '切换浅色模式' : '切换深色模式'}
          >
            {theme.mode === 'dark' ? <Sun size={17} strokeWidth={1.7} /> : <Moon size={17} strokeWidth={1.7} />}
            <span>{theme.mode === 'dark' ? '浅色模式' : '深色模式'}</span>
          </button>
          <button
            type="button"
            className="rail-collapse"
            onClick={() => setCollapsed(value => !value)}
            title={collapsed ? '展开导航' : '收起导航'}
          >
            {collapsed ? <ChevronRight size={15} /> : <ChevronLeft size={15} />}
            <span>{collapsed ? '展开' : '收起'}</span>
          </button>
        </div>
      </aside>

      <header className="page-command-bar">
        <div className="page-identity">
          <span className="page-kicker">Personal Workbench</span>
          <div className="page-title-row">
            <ActiveIcon size={18} strokeWidth={1.7} />
            <h1>{active.label}</h1>
            <span>{active.description}</span>
          </div>
        </div>
        <div className="page-command-actions">
          <span
            className={`daemon-status-chip ${daemonStatus?.healthy ? 'is-healthy' : 'is-offline'}`}
            role="status"
            aria-live="polite"
            title={daemonStatus
              ? `pwcli ${daemonStatus.version ?? ''} · ${daemonStatus.activeTasks} 个任务 · ${daemonStatus.activeAcpSessions} 个 ACP 会话`
              : 'pwcli daemon 不可用'}
          >
            <i aria-hidden="true" />
            daemon · {daemonStatus?.healthy ? '在线' : '离线'}
            {daemonStatus?.healthy ? ` · ${daemonStatus.activeTasks}/${daemonStatus.activeAcpSessions}` : ''}
          </span>
          {children}
        </div>
      </header>

      <nav className="mobile-primary-nav" aria-label="移动端主导航">
        {TABS.map(({ id, icon: Icon, label }) => (
          <button
            key={id}
            type="button"
            onClick={() => selectTab(id)}
            className={activeTab === id ? 'is-active' : ''}
            aria-current={activeTab === id ? 'page' : undefined}
          >
            <Icon size={18} strokeWidth={1.7} />
            <span>{label}</span>
          </button>
        ))}
        <button
          type="button"
          onClick={() => setMobileMoreOpen(value => !value)}
          aria-expanded={mobileMoreOpen}
          aria-label="打开更多功能"
        >
          <Menu size={18} strokeWidth={1.7} />
          <span>更多</span>
        </button>
      </nav>

      {mobileMoreOpen && (
        <div className="mobile-more-layer" role="presentation" onClick={() => setMobileMoreOpen(false)}>
          <div
            ref={mobileMoreRef}
            className="mobile-more-sheet"
            role="dialog"
            aria-modal="true"
            aria-label="更多功能"
            onClick={event => event.stopPropagation()}
          >
            <div className="mobile-more-heading">
              <strong>工具与设置</strong>
              <button type="button" onClick={() => setMobileMoreOpen(false)} aria-label="关闭">
                <X size={18} />
              </button>
            </div>
            <button type="button" className="mobile-more-item" onClick={() => {
              suppressMobileMoreRestoreRef.current = true;
              onOpenToolbox();
              setMobileMoreOpen(false);
            }}>
              <Wrench size={18} strokeWidth={1.7} />
              <span><strong>工具中心</strong><small>剪贴板、Diff 与时间转换</small></span>
            </button>
            <div className="mobile-more-separator" />
            {onOpenSettings && (
              <button type="button" className="mobile-more-item" onClick={() => {
                suppressMobileMoreRestoreRef.current = true;
                onOpenSettings();
                setMobileMoreOpen(false);
              }}>
                <Settings size={18} strokeWidth={1.7} />
                <span><strong>设置</strong><small>模型、数据与应用偏好</small></span>
              </button>
            )}
            <button type="button" className="mobile-more-item" onClick={() => { onToggleMode(); setMobileMoreOpen(false); }}>
              {theme.mode === 'dark' ? <Sun size={18} /> : <Moon size={18} />}
              <span><strong>{theme.mode === 'dark' ? '浅色模式' : '深色模式'}</strong><small>切换界面主题</small></span>
            </button>
          </div>
        </div>
      )}
    </>
  );
}
