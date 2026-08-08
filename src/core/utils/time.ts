/** Parse HH:mm to minutes since midnight; invalid formats return Infinity. */
export function parseTimeToMinutes(timeStr: string): number {
  const match = timeStr.match(/^(\d{1,2}):(\d{2})$/);
  if (!match) return Infinity;
  const h = parseInt(match[1], 10);
  const m = parseInt(match[2], 10);
  if (h > 23 || m > 59 || isNaN(h) || isNaN(m)) return Infinity;
  return h * 60 + m;
}

/** Format minutes as HH:mm. */
export function formatMinutesToTime(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`;
}

/** Normalize loose user time input (e.g. "9" → "09:00", "930" → "09:30"). */
export function normalizeTimeInput(raw: string): string {
  if (!raw.trim()) return '';
  const cleaned = raw.trim().replace(/[.,-]/g, ':');
  if (/^\d+$/.test(cleaned)) {
    const digits = cleaned.padStart(4, '0');
    const h = parseInt(digits.slice(0, 2), 10);
    const m = parseInt(digits.slice(2, 4), 10);
    if (h > 23 || m > 59) return '';
    return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`;
  }
  const match = cleaned.match(/^(\d{1,2}):(\d{1,2})$/);
  if (match) {
    const h = parseInt(match[1], 10);
    const m = parseInt(match[2], 10);
    if (h > 23 || m > 59) return '';
    return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`;
  }
  return '';
}

/** Format a millisecond duration compactly, e.g. "45s", "3m 20s", "2h 5m". */
export function formatDuration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rs = s % 60;
  if (m < 60) return `${m}m${rs > 0 ? ` ${rs}s` : ''}`;
  const h = Math.floor(m / 60);
  const rm = m % 60;
  return `${h}h${rm > 0 ? ` ${rm}m` : ''}`;
}

/** Format an ISO timestamp as Chinese relative time, e.g. "刚刚", "5分钟前". */
export function formatRelTime(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const m = Math.floor(diff / 60000);
  if (m < 1) return '刚刚';
  if (m < 60) return `${m}分钟前`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}小时前`;
  const d = Math.floor(h / 24);
  return `${d}天前`;
}

/** Format an ISO future timestamp as Chinese relative time, e.g. "即将", "5分钟后". */
export function formatFutureRelTime(iso: string): string {
  const diff = new Date(iso).getTime() - Date.now();
  const m = Math.floor(diff / 60000);
  if (m < 1) return '即将';
  if (m < 60) return `${m}分钟后`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}小时后`;
  const d = Math.floor(h / 24);
  return `${d}天后`;
}
