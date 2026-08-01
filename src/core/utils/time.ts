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
