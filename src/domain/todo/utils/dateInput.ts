export function isFourDigitDateValue(value: string): boolean {
  if (value === '') return true;
  const date = parseDateValue(value);
  return date !== undefined && formatDateValue(date) === value;
}

export function relativeDateValue(days: number, base = new Date()): string {
  const date = new Date(base.getFullYear(), base.getMonth(), base.getDate() + days);
  return formatDateValue(date);
}

export function parseDateValue(value: string): Date | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) return undefined;
  const year = Number(match[1]);
  const month = Number(match[2]) - 1;
  const day = Number(match[3]);
  if (year < 1000 || year > 9999) return undefined;
  const date = new Date(year, month, day);
  return date.getFullYear() === year && date.getMonth() === month && date.getDate() === day
    ? date
    : undefined;
}

export function formatDateValue(date: Date): string {
  return [
    String(date.getFullYear()).padStart(4, '0'),
    String(date.getMonth() + 1).padStart(2, '0'),
    String(date.getDate()).padStart(2, '0'),
  ].join('-');
}

export function addDateValue(value: string, days: number): string {
  const date = parseDateValue(value);
  return date ? relativeDateValue(days, date) : value;
}

export function shiftMonthValue(value: string, months: number): string {
  const date = parseDateValue(value);
  if (!date) return value;
  const target = new Date(date.getFullYear(), date.getMonth() + months + 1, 0);
  return formatDateValue(new Date(target.getFullYear(), target.getMonth(), Math.min(date.getDate(), target.getDate())));
}

export function nextMondayValue(base = new Date()): string {
  const day = base.getDay();
  return relativeDateValue(day === 1 ? 7 : (8 - day) % 7, base);
}

export interface CalendarDay {
  value: string;
  day: number;
  inMonth: boolean;
}

export function calendarDays(monthValue: string): CalendarDay[] {
  const month = parseDateValue(monthValue) ?? new Date();
  const first = new Date(month.getFullYear(), month.getMonth(), 1);
  const start = new Date(first.getFullYear(), first.getMonth(), 1 - first.getDay());
  return Array.from({ length: 42 }, (_, index) => {
    const date = new Date(start.getFullYear(), start.getMonth(), start.getDate() + index);
    return {
      value: formatDateValue(date),
      day: date.getDate(),
      inMonth: date.getMonth() === first.getMonth(),
    };
  });
}
