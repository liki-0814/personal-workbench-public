import { useEffect, useRef, type ReactNode } from 'react';
import { ArrowLeft, Maximize2, Minimize2, X } from 'lucide-react';
import { load, save } from '@/core/storage';
import type { ContextPaneMode } from '../types';
import './studio.css';

interface Props {
  mode: ContextPaneMode;
  title: string;
  badge?: number;
  children: ReactNode;
  onClose: () => void;
  onBack?: () => void;
  focused?: boolean;
  onFocusToggle?: () => void;
}

const WIDTH_KEY = 'chat_context_pane_width';

export default function ContextPane({ mode, title, badge = 0, children, onClose, onBack, focused = false, onFocusToggle }: Props) {
  const widthRef = useRef(load(WIDTH_KEY, 520));
  const paneRef = useRef<HTMLElement>(null);
  const dragStart = useRef<{ x: number; width: number } | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    restoreFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const pane = paneRef.current;
    if (pane) pane.style.width = `${widthRef.current}px`;
    closeRef.current?.focus({ preventScroll: true });
    return () => restoreFocusRef.current?.focus({ preventScroll: true });
  }, []);

  useEffect(() => {
    const move = (event: PointerEvent) => {
      if (!dragStart.current || !paneRef.current) return;
      const candidate = dragStart.current.width + dragStart.current.x - event.clientX;
      const main = paneRef.current.previousElementSibling as HTMLElement | null;
      const inlineMaximum = Math.max(380, dragStart.current.width + (main?.getBoundingClientRect().width ?? 520) - 520);
      const maximum = window.matchMedia('(max-width: 1440px)').matches ? Math.min(720, window.innerWidth - 48) : Math.min(920, inlineMaximum);
      const width = Math.max(380, Math.min(maximum, candidate));
      widthRef.current = width;
      paneRef.current.style.width = `${width}px`;
    };
    const stop = () => {
      if (!dragStart.current) return;
      dragStart.current = null;
      save(WIDTH_KEY, widthRef.current);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', stop);
    return () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
    };
  }, []);

  return (
    <aside ref={paneRef} className={`context-pane is-${mode} ${focused ? 'is-focused' : ''}`} aria-label={`${title}上下文工作区`}>
      <button
        type="button"
        className="context-pane-resize"
        aria-label="调整第三屏宽度"
        onPointerDown={event => {
          event.currentTarget.setPointerCapture(event.pointerId);
          dragStart.current = { x: event.clientX, width: paneRef.current?.getBoundingClientRect().width ?? widthRef.current };
        }}
      />
      <header className="context-pane-header">
        <div>{onBack && <button type="button" onClick={onBack} aria-label="返回上一层"><ArrowLeft size={14} /></button>}<strong>{title}</strong>{badge > 0 && <span>{badge}</span>}</div>
        <div className="context-pane-header-actions">
          {onFocusToggle && <button type="button" onClick={onFocusToggle} aria-label={focused ? '退出专注态' : '进入专注态'}>{focused ? <Minimize2 size={15} /> : <Maximize2 size={15} />}</button>}
          <button ref={closeRef} type="button" onClick={onClose} aria-label="收起第三屏"><X size={15} /></button>
        </div>
      </header>
      <div className="context-pane-body">{children}</div>
    </aside>
  );
}
