import {
  Fragment,
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown } from 'lucide-react';

export interface SelectOption {
  value: string;
  label: ReactNode;
  group?: string;
  disabled?: boolean;
}

interface Props {
  value: string | number;
  options: SelectOption[];
  onValueChange: (value: string) => void;
  placeholder?: string;
  density?: 'default' | 'compact';
  className?: string;
  disabled?: boolean;
  title?: string;
  ariaLabel?: string;
  menuMinWidth?: number;
  menuClassName?: string;
  optionClassName?: string;
}

const VIEWPORT_GUTTER = 8;
const MENU_GAP = 6;
const MENU_MAX_HEIGHT = 320;

export default function SelectField({
  value,
  options,
  onValueChange,
  placeholder = '请选择',
  density = 'default',
  className = '',
  disabled = false,
  title,
  ariaLabel,
  menuMinWidth = 180,
  menuClassName = '',
  optionClassName = '',
}: Props) {
  const [open, setOpen] = useState(false);
  const [menuPosition, setMenuPosition] = useState({ top: 0, left: 0, width: 0, maxHeight: MENU_MAX_HEIGHT });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const menuId = useId();
  const stringValue = String(value);
  const selectedIndex = options.findIndex(option => option.value === stringValue);
  const selected = selectedIndex >= 0 ? options[selectedIndex] : undefined;

  const updateMenuPosition = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const groupCount = new Set(options.map(option => option.group).filter(Boolean)).size;
    const desiredHeight = Math.min(MENU_MAX_HEIGHT, options.length * 36 + groupCount * 24 + 8);
    const spaceBelow = window.innerHeight - rect.bottom - VIEWPORT_GUTTER;
    const spaceAbove = rect.top - VIEWPORT_GUTTER;
    const openAbove = spaceBelow < Math.min(desiredHeight, 160) && spaceAbove > spaceBelow;
    const availableHeight = Math.max(96, (openAbove ? spaceAbove : spaceBelow) - MENU_GAP);
    const maxHeight = Math.min(MENU_MAX_HEIGHT, availableHeight);
    const width = Math.min(
      Math.max(Math.min(rect.width, 420), menuMinWidth),
      window.innerWidth - VIEWPORT_GUTTER * 2,
    );
    const left = Math.min(
      Math.max(VIEWPORT_GUTTER, rect.left),
      window.innerWidth - VIEWPORT_GUTTER - width,
    );
    const top = openAbove
      ? Math.max(VIEWPORT_GUTTER, rect.top - MENU_GAP - Math.min(desiredHeight, maxHeight))
      : rect.bottom + MENU_GAP;
    setMenuPosition({ top, left, width, maxHeight });
  }, [menuMinWidth, options]);

  useLayoutEffect(() => {
    if (!open) return;
    updateMenuPosition();
    window.addEventListener('resize', updateMenuPosition);
    window.addEventListener('scroll', updateMenuPosition, true);
    return () => {
      window.removeEventListener('resize', updateMenuPosition);
      window.removeEventListener('scroll', updateMenuPosition, true);
    };
  }, [open, updateMenuPosition]);

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => {
      optionRefs.current[Math.max(0, selectedIndex)]?.focus();
    });
    const close = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !menuRef.current?.contains(target)) setOpen(false);
    };
    document.addEventListener('pointerdown', close);
    return () => {
      window.cancelAnimationFrame(frame);
      document.removeEventListener('pointerdown', close);
    };
  }, [open, selectedIndex]);

  const closeAndFocus = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };

  const handleMenuKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      closeAndFocus();
      return;
    }
    if (event.key === 'Tab') {
      setOpen(false);
      triggerRef.current?.focus();
      return;
    }
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const enabled = optionRefs.current.filter((option): option is HTMLButtonElement => Boolean(option && !option.disabled));
    if (enabled.length === 0) return;
    const current = enabled.findIndex(option => option === document.activeElement);
    const next = event.key === 'Home'
      ? 0
      : event.key === 'End'
        ? enabled.length - 1
        : event.key === 'ArrowDown'
          ? (current + 1) % enabled.length
          : (current - 1 + enabled.length) % enabled.length;
    enabled[next]?.focus();
  };

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        disabled={disabled}
        title={title}
        className={`input-field pwb-select-trigger ${density === 'compact' ? 'pwb-select-compact' : ''} ${className}`}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => setOpen(current => !current)}
        onKeyDown={event => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            setOpen(true);
          }
          if (event.key === 'Escape' && open) {
            event.preventDefault();
            event.stopPropagation();
            closeAndFocus();
          }
        }}
      >
        <span className={`min-w-0 flex-1 truncate text-left ${selected ? '' : 'pwb-select-placeholder'}`}>
          {selected?.label ?? placeholder}
        </span>
        <ChevronDown size={density === 'compact' ? 12 : 14} className={open ? 'rotate-180' : ''} />
      </button>

      {open && createPortal(
        <div
          ref={menuRef}
          id={menuId}
          role="listbox"
          className={`pwb-select-menu ${menuClassName}`}
          style={menuPosition}
          onKeyDown={handleMenuKeyDown}
        >
          {options.map((option, index) => {
            const showGroup = option.group && (index === 0 || options[index - 1]?.group !== option.group);
            const isSelected = option.value === stringValue;
            return (
              <Fragment key={`${option.group ?? ''}:${option.value}`}>
                {showGroup && <div className="pwb-select-group">{option.group}</div>}
                <button
                  ref={node => { optionRefs.current[index] = node; }}
                  type="button"
                  role="option"
                  disabled={option.disabled}
                  aria-selected={isSelected}
                  className={`pwb-select-option ${optionClassName} ${isSelected ? 'is-selected' : ''}`}
                  onClick={() => {
                    onValueChange(option.value);
                    closeAndFocus();
                  }}
                >
                  <span className="min-w-0 flex-1 truncate">{option.label}</span>
                  {isSelected && <Check size={14} />}
                </button>
              </Fragment>
            );
          })}
        </div>,
        document.body,
      )}
    </>
  );
}
