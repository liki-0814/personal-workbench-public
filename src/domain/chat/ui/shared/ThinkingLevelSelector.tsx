import {
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { createPortal } from 'react-dom';
import { ChevronDown, Zap } from 'lucide-react';
import { THINKING_LEVEL_LABELS, type ThinkingLevel } from '@/core/config';
import './ThinkingLevelSelector.css';

const GUIDANCE: Record<ThinkingLevel, string> = {
  off: '使用模型默认速度，不额外请求扩展推理。',
  minimal: '最快的轻量推理，适合简单确认和短回答。',
  low: '较快，适合日常问答和小范围修改。',
  medium: '速度与推理质量平衡，适合大多数任务。',
  high: '适合复杂分析与多步骤任务，响应可能更慢。',
  xhigh: '投入更多推理预算，适合高难度问题。',
  max: '使用模型支持的最大常规推理强度。',
  ultra: '最高可用强度，耗时和 token 消耗可能显著增加。',
};

const POPOVER_WIDTH = 280;
const POPOVER_HEIGHT = 196;
const VIEWPORT_GUTTER = 8;
const POPOVER_GAP = 7;

export default function ThinkingLevelSelector({
  value,
  levels,
  onChange,
  disabled,
  pending = false,
}: {
  value: ThinkingLevel;
  levels: ThinkingLevel[];
  onChange: (level: ThinkingLevel) => void;
  disabled?: boolean;
  pending?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [keyboardFocus, setKeyboardFocus] = useState(false);
  const [position, setPosition] = useState({ top: 0, left: 0, width: POPOVER_WIDTH });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const rangeRef = useRef<HTMLInputElement>(null);
  const dragPointerRef = useRef<number | null>(null);
  const headingId = useId();
  const selectedIndex = Math.max(0, levels.indexOf(value));
  const maxIndex = Math.max(0, levels.length - 1);
  const progress = maxIndex === 0 ? 0 : selectedIndex / maxIndex;
  const currentLevel = levels[selectedIndex] ?? value;
  const currentLabel = THINKING_LEVEL_LABELS[currentLevel];

  const updatePosition = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const width = Math.min(POPOVER_WIDTH, window.innerWidth - VIEWPORT_GUTTER * 2);
    const left = Math.min(
      Math.max(VIEWPORT_GUTTER, rect.right - width),
      window.innerWidth - width - VIEWPORT_GUTTER,
    );
    const openAbove = rect.bottom + POPOVER_GAP + POPOVER_HEIGHT > window.innerHeight - VIEWPORT_GUTTER;
    const top = openAbove
      ? Math.max(VIEWPORT_GUTTER, rect.top - POPOVER_GAP - POPOVER_HEIGHT)
      : rect.bottom + POPOVER_GAP;
    setPosition({ top, left, width });
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    updatePosition();
    window.addEventListener('resize', updatePosition);
    window.addEventListener('scroll', updatePosition, true);
    return () => {
      window.removeEventListener('resize', updatePosition);
      window.removeEventListener('scroll', updatePosition, true);
    };
  }, [open, updatePosition]);

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => rangeRef.current?.focus());
    const close = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !popoverRef.current?.contains(target)) {
        setOpen(false);
      }
    };
    document.addEventListener('pointerdown', close);
    return () => {
      window.cancelAnimationFrame(frame);
      document.removeEventListener('pointerdown', close);
    };
  }, [open]);

  useEffect(() => {
    if (!dragging) return;
    const stopDragging = () => setDragging(false);
    window.addEventListener('pointerup', stopDragging);
    window.addEventListener('pointercancel', stopDragging);
    return () => {
      window.removeEventListener('pointerup', stopDragging);
      window.removeEventListener('pointercancel', stopDragging);
    };
  }, [dragging]);

  const chooseIndex = (index: number) => {
    const next = levels[Math.min(maxIndex, Math.max(0, index))];
    if (next && next !== value) onChange(next);
  };

  const chooseFromPointer = (event: ReactPointerEvent<HTMLInputElement>) => {
    if (maxIndex === 0) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const thumbInset = 15;
    const trackWidth = Math.max(1, rect.width - thumbInset * 2);
    const ratio = Math.min(1, Math.max(0, (event.clientX - rect.left - thumbInset) / trackWidth));
    chooseIndex(Math.round(ratio * maxIndex));
  };

  const handleRangeKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    let next: number | undefined;
    if (event.key === 'ArrowRight' || event.key === 'ArrowUp') next = selectedIndex + 1;
    if (event.key === 'ArrowLeft' || event.key === 'ArrowDown') next = selectedIndex - 1;
    if (event.key === 'Home') next = 0;
    if (event.key === 'End') next = maxIndex;
    if (event.key === 'Escape') {
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.focus();
      return;
    }
    if (next === undefined) return;
    event.preventDefault();
    chooseIndex(next);
  };

  const sliderStyle = {
    '--thinking-progress': `${progress * 100}%`,
  } as CSSProperties;

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        disabled={disabled || levels.length === 0}
        className="input-field pwb-select-trigger pwb-select-compact precision-model-trigger thinking-level-trigger"
        title={`${pending ? '下一次模型调用' : '思考深度'}：${currentLabel}`}
        aria-label={`设置思考深度，${pending ? '下一次模型调用将使用' : '当前为'}${currentLabel}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => {
          setKeyboardFocus(false);
          setOpen(current => !current);
        }}
        onKeyDown={event => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            setKeyboardFocus(true);
            setOpen(true);
          }
          if (event.key === 'Escape') setOpen(false);
        }}
      >
        <Zap size={12} className="thinking-level-trigger-icon" aria-hidden="true" />
        <span>{pending ? '下一步' : '思考'}：{currentLabel}</span>
        <ChevronDown size={12} className={open ? 'rotate-180' : ''} aria-hidden="true" />
      </button>

      {open && createPortal(
        <div
          ref={popoverRef}
          role="dialog"
          aria-labelledby={headingId}
          className="thinking-level-popover"
          style={{ top: position.top, left: position.left, width: position.width }}
        >
          <div className="thinking-level-heading">
            <div>
              <span id={headingId}>思考深度</span>
              <strong>{currentLabel}</strong>
            </div>
            <Zap size={15} aria-hidden="true" />
          </div>

          <div
            className={`thinking-level-slider ${dragging ? 'is-dragging' : ''} ${keyboardFocus ? 'is-keyboard-focus' : ''}`}
            style={sliderStyle}
          >
            <div className="thinking-level-track" aria-hidden="true">
              <div className="thinking-level-fill" />
              {levels.map((level, index) => (
                <span
                  key={level}
                  className={`thinking-level-marker ${index <= selectedIndex ? 'is-active' : ''}`}
                  style={{ left: `${maxIndex === 0 ? 50 : (index / maxIndex) * 100}%` }}
                />
              ))}
              <span className="thinking-level-thumb" />
            </div>
            <input
              ref={rangeRef}
              className="thinking-level-range"
              type="range"
              min={0}
              max={maxIndex}
              step={1}
              value={selectedIndex}
              disabled={levels.length <= 1}
              aria-label="思考深度"
              aria-valuetext={currentLabel}
              onChange={event => chooseIndex(Number(event.currentTarget.value))}
              onKeyDown={event => {
                setKeyboardFocus(true);
                handleRangeKeyDown(event);
              }}
              onPointerDown={event => {
                setKeyboardFocus(false);
                dragPointerRef.current = event.pointerId;
                event.currentTarget.focus();
                event.currentTarget.setPointerCapture?.(event.pointerId);
                setDragging(true);
                chooseFromPointer(event);
              }}
              onPointerMove={event => {
                if (dragPointerRef.current !== event.pointerId) return;
                chooseFromPointer(event);
              }}
              onPointerUp={event => {
                if (dragPointerRef.current === event.pointerId) chooseFromPointer(event);
                dragPointerRef.current = null;
                event.currentTarget.releasePointerCapture?.(event.pointerId);
                setDragging(false);
              }}
              onPointerCancel={event => {
                if (dragPointerRef.current === event.pointerId) dragPointerRef.current = null;
                setDragging(false);
              }}
              onBlur={() => {
                setDragging(false);
                setKeyboardFocus(false);
              }}
            />
          </div>

          <div
            className="thinking-level-labels"
            style={{ '--thinking-level-count': levels.length } as CSSProperties}
            aria-hidden="true"
          >
            {levels.map((level, index) => (
              <span key={level} className={index === selectedIndex ? 'is-active' : ''}>
                {THINKING_LEVEL_LABELS[level]}
              </span>
            ))}
          </div>
          <p className="thinking-level-guidance" aria-live="polite">
            {pending && <strong>将在下一次模型调用生效。 </strong>}
            {GUIDANCE[currentLevel]}
          </p>
        </div>,
        document.body,
      )}
    </>
  );
}
