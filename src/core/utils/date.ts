/** Format a Date as YYYY-MM-DD. */
export function formatDate(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

/** Current ISO timestamp. */
export function now(): string {
  return new Date().toISOString();
}

/** Format as yyyyMMdd hh:mm:ss (for pomodoro records). */
export function formatDateTime(d: Date): string {
  const yyyy = d.getFullYear();
  const MM = String(d.getMonth() + 1).padStart(2, '0');
  const dd = String(d.getDate()).padStart(2, '0');
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  const ss = String(d.getSeconds()).padStart(2, '0');
  return `${yyyy}${MM}${dd} ${hh}:${mm}:${ss}`;
}

/** Last N days as YYYY-MM-DD strings (newest last). */
export function getLastNDays(n: number): string[] {
  const days: string[] = [];
  for (let i = n - 1; i >= 0; i--) {
    const d = new Date();
    d.setDate(d.getDate() - i);
    days.push(formatDate(d));
  }
  return days;
}

/** Chinese day-of-week label. */
export function getDayOfWeek(date: Date): string {
  const days = ['星期日', '星期一', '星期二', '星期三', '星期四', '星期五', '星期六'];
  return days[date.getDay()];
}

/** Number of days in a given month. */
export function getDaysInMonth(year: number, month: number): number {
  return new Date(year, month + 1, 0).getDate();
}

/** Day of week for the first day of a month (0 = Sunday). */
export function getFirstDayOfMonth(year: number, month: number): number {
  return new Date(year, month, 1).getDay();
}

/** All dates in the week containing the given date (Monday → Sunday). */
export function getWeekDates(date: Date): Date[] {
  const day = date.getDay();
  const diff = day === 0 ? 6 : day - 1;
  const monday = new Date(date);
  monday.setDate(date.getDate() - diff);
  monday.setHours(0, 0, 0, 0);
  const days: Date[] = [];
  for (let i = 0; i < 7; i++) {
    const d = new Date(monday);
    d.setDate(monday.getDate() + i);
    days.push(d);
  }
  return days;
}

/** Whether two dates fall on the same calendar day. */
export function isSameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

/** All date strings within an inclusive range. */
export function getDateRange(startStr: string, endStr: string): string[] {
  const result: string[] = [];
  const start = new Date(startStr);
  const end = new Date(endStr);
  const cur = new Date(start);
  while (cur <= end) {
    result.push(formatDate(cur));
    cur.setDate(cur.getDate() + 1);
  }
  return result;
}
