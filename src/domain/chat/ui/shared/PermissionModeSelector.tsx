import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Hand, ShieldCheck, ShieldAlert } from 'lucide-react';
import {
  saveLocalConfig,
  useLocalConfig,
  type AgentPermissionMode,
} from '@/core/config';
import { showToast } from '@/shell';

const MODES: Array<{
  value: AgentPermissionMode;
  label: string;
  description: string;
  icon: typeof Hand;
  warning?: boolean;
}> = [
  {
    value: 'prompt',
    label: '请求批准',
    description: '修改数据、文件或访问互联网时始终询问',
    icon: Hand,
  },
  {
    value: 'risk',
    label: '替我审批',
    description: '仅对删除、敏感配置等风险操作请求批准',
    icon: ShieldCheck,
  },
  {
    value: 'full',
    label: '完全访问权限',
    description: '自动执行操作；危险命令仍受硬性安全规则限制',
    icon: ShieldAlert,
    warning: true,
  },
];

export default function PermissionModeSelector() {
  const config = useLocalConfig();
  const mode = config.permissions?.agent_mode ?? 'risk';
  const selected = MODES.find(item => item.value === mode) ?? MODES[1];
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current) return;
    const update = () => {
      const rect = triggerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const width = Math.min(390, window.innerWidth - 16);
      const left = Math.min(Math.max(8, rect.right - width), window.innerWidth - width - 8);
      const estimatedHeight = 210;
      const top = rect.bottom + estimatedHeight > window.innerHeight - 8
        ? Math.max(8, rect.top - estimatedHeight - 6)
        : rect.bottom + 6;
      setPosition({ top, left });
    };
    update();
    window.addEventListener('resize', update);
    window.addEventListener('scroll', update, true);
    return () => {
      window.removeEventListener('resize', update);
      window.removeEventListener('scroll', update, true);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !menuRef.current?.contains(target)) {
        setOpen(false);
      }
    };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [open]);

  const choose = async (nextMode: AgentPermissionMode) => {
    setOpen(false);
    if (nextMode === mode) return;
    setSaving(true);
    try {
      await saveLocalConfig({ permissions: { agent_mode: nextMode } });
    } catch (error) {
      showToast({ message: error instanceof Error ? error.message : '权限模式保存失败', type: 'error' });
    } finally {
      setSaving(false);
    }
  };

  const SelectedIcon = selected.icon;
  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className="pwb-pill-btn precision-tool-button"
        disabled={saving}
        title={`权限模式：${selected.label}`}
        aria-label={`权限模式：${selected.label}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen(value => !value)}
      >
        <SelectedIcon size={13} />
        <span>{selected.label}</span>
        <ChevronDown size={11} className={open ? 'rotate-180' : ''} />
      </button>
      {open && createPortal(
        <div
          ref={menuRef}
          role="listbox"
          aria-label="Agent 权限模式"
          className="pwb-select-menu"
          style={{ top: position.top, left: position.left, width: 'min(390px, calc(100vw - 16px))' }}
          onKeyDown={event => {
            if (event.key === 'Escape') {
              event.preventDefault();
              setOpen(false);
              triggerRef.current?.focus();
            }
          }}
        >
          {MODES.map(item => {
            const Icon = item.icon;
            const active = item.value === mode;
            return (
              <button
                key={item.value}
                type="button"
                role="option"
                aria-selected={active}
                className={`pwb-select-option min-h-[58px] items-start ${active ? 'is-selected' : ''}`}
                onClick={() => void choose(item.value)}
              >
                <Icon
                  size={18}
                  className={`mt-0.5 ${item.warning ? 'text-orange-500' : ''}`}
                />
                <span className="min-w-0 flex-1">
                  <span className={`block text-sm font-semibold ${item.warning ? 'text-orange-500' : ''}`}>
                    {item.label}
                  </span>
                  <span className={`mt-0.5 block text-xs font-normal leading-4 ${item.warning ? 'text-orange-500' : 'text-[var(--text-muted)]'}`}>
                    {item.description}
                  </span>
                </span>
                {active && <Check size={15} className="mt-1" />}
              </button>
            );
          })}
        </div>,
        document.body,
      )}
    </>
  );
}
