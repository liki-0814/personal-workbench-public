import { describe, expect, it } from 'vitest';
import {
  formatDate,
  formatDateTime,
  getDayOfWeek,
  getDaysInMonth,
  getFirstDayOfMonth,
  getWeekDates,
  isSameDay,
  getDateRange,
} from '@/core/utils/date';

describe('formatDate', () => {
  it('formats single-digit month and day with leading zeros', () => {
    expect(formatDate(new Date(2026, 0, 5))).toBe('2026-01-05');
  });

  it('formats double-digit month and day', () => {
    expect(formatDate(new Date(2026, 11, 25))).toBe('2026-12-25');
  });
});

describe('formatDateTime', () => {
  it('formats as yyyyMMdd hh:mm:ss', () => {
    const d = new Date(2026, 5, 20, 14, 5, 9);
    expect(formatDateTime(d)).toBe('20260620 14:05:09');
  });

  it('pads single-digit values', () => {
    const d = new Date(2026, 0, 1, 1, 2, 3);
    expect(formatDateTime(d)).toBe('20260101 01:02:03');
  });
});

describe('getDayOfWeek', () => {
  it('returns Chinese day labels', () => {
    expect(getDayOfWeek(new Date(2026, 5, 15))).toBe('星期一');
    expect(getDayOfWeek(new Date(2026, 5, 20))).toBe('星期六');
    expect(getDayOfWeek(new Date(2026, 5, 21))).toBe('星期日');
  });
});

describe('getDaysInMonth', () => {
  it('handles February in a non-leap year', () => {
    expect(getDaysInMonth(2025, 1)).toBe(28);
  });

  it('handles February in a leap year', () => {
    expect(getDaysInMonth(2024, 1)).toBe(29);
  });

  it('handles 31-day months', () => {
    expect(getDaysInMonth(2026, 0)).toBe(31);
    expect(getDaysInMonth(2026, 6)).toBe(31);
  });

  it('handles 30-day months', () => {
    expect(getDaysInMonth(2026, 3)).toBe(30);
    expect(getDaysInMonth(2026, 5)).toBe(30);
  });
});

describe('getFirstDayOfMonth', () => {
  it('returns day of week for first of month', () => {
    // June 2026 starts on Monday
    expect(getFirstDayOfMonth(2026, 5)).toBe(1);
  });
});

describe('getWeekDates', () => {
  it('returns Monday through Sunday for a mid-week date', () => {
    const wed = new Date(2026, 5, 17); // Wednesday
    const week = getWeekDates(wed);
    expect(week).toHaveLength(7);
    expect(week[0].getDay()).toBe(1); // Monday
    expect(week[6].getDay()).toBe(0); // Sunday
    expect(formatDate(week[0])).toBe('2026-06-15');
    expect(formatDate(week[6])).toBe('2026-06-21');
  });

  it('handles Sunday correctly (last day of week)', () => {
    const sun = new Date(2026, 5, 21);
    const week = getWeekDates(sun);
    expect(formatDate(week[0])).toBe('2026-06-15');
    expect(formatDate(week[6])).toBe('2026-06-21');
  });

  it('handles Monday correctly (first day of week)', () => {
    const mon = new Date(2026, 5, 15);
    const week = getWeekDates(mon);
    expect(formatDate(week[0])).toBe('2026-06-15');
  });
});

describe('isSameDay', () => {
  it('returns true for same calendar day', () => {
    expect(isSameDay(new Date(2026, 5, 20, 0, 0), new Date(2026, 5, 20, 23, 59))).toBe(true);
  });

  it('returns false for different days', () => {
    expect(isSameDay(new Date(2026, 5, 20), new Date(2026, 5, 21))).toBe(false);
  });

  it('returns false for same day different month', () => {
    expect(isSameDay(new Date(2026, 4, 20), new Date(2026, 5, 20))).toBe(false);
  });
});

describe('getDateRange', () => {
  it('returns inclusive range of date strings', () => {
    const range = getDateRange('2026-06-18', '2026-06-21');
    expect(range).toEqual(['2026-06-18', '2026-06-19', '2026-06-20', '2026-06-21']);
  });

  it('returns single date when start equals end', () => {
    expect(getDateRange('2026-06-20', '2026-06-20')).toEqual(['2026-06-20']);
  });

  it('returns empty array when end is before start', () => {
    expect(getDateRange('2026-06-20', '2026-06-18')).toEqual([]);
  });

  it('handles month boundaries', () => {
    const range = getDateRange('2026-06-29', '2026-07-02');
    expect(range).toEqual(['2026-06-29', '2026-06-30', '2026-07-01', '2026-07-02']);
  });
});
