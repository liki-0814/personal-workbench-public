import { describe, expect, it } from 'vitest';
import { clampDuration, clampStart, formatMinute, localDateKey, parseTime, plansForToday } from '@/domain/day-plan/utils';

describe('day plan utilities', () => {
  it('uses the local calendar date instead of UTC', () => {
    expect(localDateKey(new Date(2026, 6, 13, 23, 30))).toBe('2026-07-13');
  });

  it('parses, formats and snaps time safely', () => {
    expect(parseTime('9:05')).toBe(545);
    expect(parseTime('24:00')).toBeNull();
    expect(formatMinute(545)).toBe('09:05');
    expect(clampStart(548, 30)).toBe(550);
    expect(formatMinute(24 * 60)).toBe('24:00');
  });

  it('snaps duration to half hours and truncates at the end of the day', () => {
    expect(clampDuration(0.1 * 60)).toBe(30);
    expect(clampDuration(12 * 60)).toBe(12 * 60);
    expect(clampDuration(1.2 * 60)).toBe(60);
    expect(clampDuration(12 * 60, 20 * 60)).toBe(4 * 60);
    expect(clampDuration(2 * 60, 23.5 * 60)).toBe(30);
    expect(clampStart(7 * 60)).toBe(8 * 60);
  });

  it('keeps only today and sorts by start time', () => {
    const base = { title: 'x', durationMinutes: 30, createdAt: '', updatedAt: '' };
    const plans = [
      { ...base, id: 'later', date: '2026-07-13', startMinute: 600 },
      { ...base, id: 'old', date: '2026-07-12', startMinute: 500 },
      { ...base, id: 'early', date: '2026-07-13', startMinute: 480 },
    ];
    expect(plansForToday(plans, new Date(2026, 6, 13)).map(item => item.id)).toEqual(['early', 'later']);
  });
});
