import type { TimestampToolState } from '../types';

export interface TimestampResult {
  date: Date;
  seconds: string;
  milliseconds: string;
  local: string;
  utc: string;
  iso: string;
}

const UNIT_DIVISOR: Record<Exclude<TimestampToolState['unit'], 'auto'>, bigint> = {
  seconds: 1n,
  milliseconds: 1_000n,
  microseconds: 1_000_000n,
  nanoseconds: 1_000_000_000n,
};

function detectUnit(value: string): Exclude<TimestampToolState['unit'], 'auto'> {
  const digits = value.replace(/^[+-]/, '').length;
  if (digits <= 10) return 'seconds';
  if (digits <= 13) return 'milliseconds';
  if (digits <= 16) return 'microseconds';
  return 'nanoseconds';
}

export function parseTimestamp(
  input: string,
  unit: TimestampToolState['unit'],
): TimestampResult | null {
  const normalized = input.trim();
  if (!/^[+-]?\d+$/.test(normalized)) return null;

  try {
    const source = BigInt(normalized);
    const resolvedUnit = unit === 'auto' ? detectUnit(normalized) : unit;
    const milliseconds = resolvedUnit === 'seconds'
      ? source * 1_000n
      : source / (UNIT_DIVISOR[resolvedUnit] / 1_000n);
    const millisecondsNumber = Number(milliseconds);
    if (!Number.isSafeInteger(millisecondsNumber)) return null;
    const date = new Date(millisecondsNumber);
    if (Number.isNaN(date.getTime())) return null;
    const localBase = new Intl.DateTimeFormat('zh-CN', {
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: false,
    }).format(date);

    return {
      date,
      seconds: Math.floor(millisecondsNumber / 1_000).toString(),
      milliseconds: millisecondsNumber.toString(),
      local: `${localBase}.${String(date.getMilliseconds()).padStart(3, '0')}`,
      utc: date.toUTCString(),
      iso: date.toISOString(),
    };
  } catch {
    return null;
  }
}
