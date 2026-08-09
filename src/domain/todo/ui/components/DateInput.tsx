import { useEffect, useMemo, useRef, useState } from 'react';
import { CalendarDays, ChevronLeft, ChevronRight } from 'lucide-react';
import {
  addDateValue,
  calendarDays,
  isFourDigitDateValue,
  nextMondayValue,
  parseDateValue,
  relativeDateValue,
  shiftMonthValue,
} from '../../utils/dateInput';

interface Props {
  value: string;
  onChange: (value: string) => void;
  onBlur?: () => void;
  ariaLabel: string;
  className: string;
  min?: string;
  quickOptions?: boolean;
}

const WEEKDAYS = ['日', '一', '二', '三', '四', '五', '六'];
const MAX_DATE = '9999-12-31';

function longDateLabel(value: string): string {
  const date = parseDateValue(value);
  if (!date) return '选择日期';
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
    weekday: 'short',
  }).format(date);
}

function monthLabel(value: string): string {
  const date = parseDateValue(value);
  return date
    ? new Intl.DateTimeFormat('zh-CN', { year: 'numeric', month: 'long' }).format(date)
    : '';
}

export default function DateInput({
  value,
  onChange,
  onBlur,
  ariaLabel,
  className,
  min = '1000-01-01',
  quickOptions = false,
}: Props) {
  const today = relativeDateValue(0);
  const initialDate = value || today;
  const [open, setOpen] = useState(false);
  const [textValue, setTextValue] = useState(value);
  const [focusedValue, setFocusedValue] = useState(initialDate);
  const [viewMonth, setViewMonth] = useState(initialDate);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const focusedDayRef = useRef<HTMLButtonElement>(null);
  const focusGridRef = useRef(false);
  const suppressOpenOnFocusRef = useRef(false);
  const days = useMemo(() => calendarDays(viewMonth), [viewMonth]);

  useEffect(() => {
    setTextValue(value);
    if (!value) return;
    setFocusedValue(value);
    setViewMonth(value);
  }, [value]);

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => {
      if (wrapperRef.current?.contains(event.target as Node)) return;
      setOpen(false);
      setTextValue(value);
    };
    window.addEventListener('pointerdown', close);
    return () => window.removeEventListener('pointerdown', close);
  }, [onBlur, open, value]);

  useEffect(() => {
    if (open && focusGridRef.current) focusedDayRef.current?.focus({ preventScroll: true });
  }, [focusedValue, open, viewMonth]);

  const isSelectable = (dateValue: string) => dateValue >= min && dateValue <= MAX_DATE;

  const openCalendar = (focusGrid: boolean) => {
    const anchor = value || today;
    focusGridRef.current = focusGrid;
    setFocusedValue(anchor);
    setViewMonth(anchor);
    setOpen(true);
  };

  const selectDate = (dateValue: string) => {
    if (!isSelectable(dateValue)) return;
    onChange(dateValue);
    setTextValue(dateValue);
    setFocusedValue(dateValue);
    setViewMonth(dateValue);
    setOpen(false);
    focusGridRef.current = false;
  };

  const moveFocus = (dateValue: string) => {
    if (!isSelectable(dateValue)) return;
    focusGridRef.current = true;
    setFocusedValue(dateValue);
    setViewMonth(dateValue);
  };

  const clearDate = () => {
    onChange('');
    setTextValue('');
    setOpen(false);
    focusGridRef.current = false;
  };

  const quickDates = [
    { label: '今天', value: today },
    { label: '明天', value: relativeDateValue(1) },
    { label: '下周一', value: nextMondayValue() },
    { label: '一周后', value: relativeDateValue(7) },
  ];

  return (
    <div
      ref={wrapperRef}
      className="relative"
      onBlurCapture={event => {
        if (wrapperRef.current?.contains(event.relatedTarget as Node | null)) return;
        setOpen(false);
        setTextValue(value);
        onBlur?.();
      }}
    >
      <div className="relative">
        <input
          ref={inputRef}
          type="text"
          inputMode="numeric"
          value={textValue}
          onFocus={() => {
            if (suppressOpenOnFocusRef.current) {
              suppressOpenOnFocusRef.current = false;
              return;
            }
            openCalendar(false);
          }}
          onChange={event => {
            const nextValue = event.target.value.replace(/[^\d-]/g, '').slice(0, 10);
            setTextValue(nextValue);
            if (nextValue === '') {
              onChange('');
            } else if (isFourDigitDateValue(nextValue) && isSelectable(nextValue)) {
              onChange(nextValue);
              setFocusedValue(nextValue);
              setViewMonth(nextValue);
            }
          }}
          onKeyDown={event => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === 'ArrowDown') {
              event.preventDefault();
              openCalendar(true);
            } else if (event.key === 'Enter') {
              event.preventDefault();
              if (isFourDigitDateValue(textValue) && textValue && isSelectable(textValue)) {
                selectDate(textValue);
              } else {
                setTextValue(value);
              }
            } else if (event.key === 'Escape') {
              event.stopPropagation();
              setOpen(false);
              setTextValue(value);
              inputRef.current?.blur();
            }
          }}
          placeholder="YYYY-MM-DD"
          className={`${className} pr-10 font-mono tabular-nums`}
          aria-label={ariaLabel}
          aria-expanded={open}
          aria-haspopup="dialog"
        />
        <button
          type="button"
          onClick={() => {
            if (open) {
              setOpen(false);
              return;
            }
            openCalendar(true);
          }}
          className="absolute right-1.5 top-1/2 flex h-7 w-7 -translate-y-1/2 items-center justify-center rounded-[7px] text-[#7A818C] transition-colors hover:bg-[#E9EBEF] hover:text-[#30353D] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 dark:text-[#8D96A3] dark:hover:bg-[#2A3039] dark:hover:text-[#D9DEE6]"
          aria-label={`打开${ariaLabel}日历`}
        >
          <CalendarDays size={14} />
        </button>
      </div>

      {open && (
        <div
          role="dialog"
          aria-label={`${ariaLabel}日历`}
          className="mt-2 w-full rounded-[14px] border border-[#D9DDE4] bg-white p-3.5 shadow-[0_10px_28px_rgba(17,22,56,0.08)] dark:border-[#343B45] dark:bg-[#181B21] dark:shadow-none"
        >
          <div className="mb-3">
            <div className="text-[13px] font-semibold text-[#30353D] dark:text-[#E1E5EB]">
              {longDateLabel(value || focusedValue)}
            </div>
            <div className="mt-0.5 text-[9px] text-[#8A919C] dark:text-[#7E8794]">点击日期即保存</div>
          </div>

          {quickOptions && (
            <div className="mb-3 flex items-center gap-1 border-b border-[#ECEEF1] pb-3 dark:border-[#2A3039]">
              {quickDates.map(option => (
                <button
                  key={option.label}
                  type="button"
                  onClick={() => selectDate(option.value)}
                  className="rounded-[6px] px-2 py-1 text-[10px] text-[#69717D] transition-colors hover:bg-[#EEF1FC] hover:text-brand focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 dark:text-[#98A1AD] dark:hover:bg-[#202639] dark:hover:text-[#AEBBFF]"
                >
                  {option.label}
                </button>
              ))}
            </div>
          )}

          <div className="mb-2 flex items-center justify-between">
            <button
              type="button"
              onClick={() => {
                const previous = shiftMonthValue(viewMonth, -1);
                setViewMonth(previous);
                setFocusedValue(previous);
              }}
              className="flex h-7 w-7 items-center justify-center rounded-[7px] text-[#7A818C] hover:bg-[#F0F1F3] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 dark:text-[#8D96A3] dark:hover:bg-[#252A32]"
              aria-label="上个月"
            >
              <ChevronLeft size={14} />
            </button>
            <span className="text-[11px] font-semibold text-[#4B535E] dark:text-[#C8CED7]">{monthLabel(viewMonth)}</span>
            <button
              type="button"
              onClick={() => {
                const next = shiftMonthValue(viewMonth, 1);
                setViewMonth(next);
                setFocusedValue(next);
              }}
              className="flex h-7 w-7 items-center justify-center rounded-[7px] text-[#7A818C] hover:bg-[#F0F1F3] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 dark:text-[#8D96A3] dark:hover:bg-[#252A32]"
              aria-label="下个月"
            >
              <ChevronRight size={14} />
            </button>
          </div>

          <div className="grid grid-cols-7 text-center">
            {WEEKDAYS.map(day => (
              <span key={day} className="pb-1.5 text-[9px] font-medium text-[#A0A6B0] dark:text-[#69727F]">{day}</span>
            ))}
            {days.map(day => {
              const selected = day.value === value;
              const focused = day.value === focusedValue;
              const isToday = day.value === today;
              const disabled = !isSelectable(day.value);
              return (
                <button
                  key={day.value}
                  ref={focused ? focusedDayRef : undefined}
                  type="button"
                  tabIndex={focused ? 0 : -1}
                  disabled={disabled}
                  onClick={() => selectDate(day.value)}
                  onKeyDown={event => {
                    let nextValue: string | undefined;
                    if (event.key === 'ArrowLeft') nextValue = addDateValue(focusedValue, -1);
                    if (event.key === 'ArrowRight') nextValue = addDateValue(focusedValue, 1);
                    if (event.key === 'ArrowUp') nextValue = addDateValue(focusedValue, -7);
                    if (event.key === 'ArrowDown') nextValue = addDateValue(focusedValue, 7);
                    if (event.key === 'PageUp') nextValue = shiftMonthValue(focusedValue, -1);
                    if (event.key === 'PageDown') nextValue = shiftMonthValue(focusedValue, 1);
                    if (event.key === 'Enter' || event.key === ' ') {
                      event.preventDefault();
                      selectDate(focusedValue);
                      return;
                    }
                    if (event.key === 'Escape') {
                      event.preventDefault();
                      event.stopPropagation();
                      setOpen(false);
                      focusGridRef.current = false;
                      suppressOpenOnFocusRef.current = true;
                      inputRef.current?.focus({ preventScroll: true });
                      return;
                    }
                    if (!nextValue) return;
                    event.preventDefault();
                    moveFocus(nextValue);
                  }}
                  className={`relative mx-auto flex h-8 w-8 items-center justify-center rounded-full text-[10px] transition-colors disabled:opacity-20 ${
                    selected
                      ? 'bg-[#20242B] font-semibold text-white shadow-[0_3px_8px_rgba(32,36,43,0.18)] dark:bg-[#E9ECF1] dark:text-[#171A1F]'
                      : focused
                        ? 'bg-[#EEF1FC] font-medium text-brand dark:bg-[#202639] dark:text-[#AEBBFF]'
                        : day.inMonth
                          ? 'text-[#4B535E] hover:bg-[#F0F1F3] dark:text-[#C8CED7] dark:hover:bg-[#252A32]'
                          : 'text-[#BEC3CB] hover:bg-[#F5F6F7] dark:text-[#555E6A] dark:hover:bg-[#22272E]'
                  } focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/55`}
                  aria-label={longDateLabel(day.value)}
                  aria-current={isToday ? 'date' : undefined}
                  aria-pressed={selected}
                >
                  {day.day}
                  {isToday && !selected && <span className="absolute bottom-[3px] h-0.5 w-0.5 rounded-full bg-[#5268D9] dark:bg-[#AEBBFF]" />}
                </button>
              );
            })}
          </div>

          <div className="mt-2 flex items-center justify-between border-t border-[#ECEEF1] pt-2.5 dark:border-[#2A3039]">
            <span className="font-mono text-[9px] text-[#A0A6B0] dark:text-[#69727F]">YYYY-MM-DD</span>
            {value && (
              <button
                type="button"
                onClick={clearDate}
                className="rounded-[6px] px-2 py-1 text-[10px] text-[#8A919C] hover:bg-[#FBEFEF] hover:text-[#A33D3D] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 dark:text-[#7E8794] dark:hover:bg-[#2C2023] dark:hover:text-[#F19A9A]"
              >
                清除日期
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
