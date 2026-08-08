import { describe, expect, it } from 'vitest';
import type { HabitFrequency, HabitRecord } from './types';
import {
  computeStreak,
  computeWeekStatus,
  getDayOfWeekForDate,
  isDueOnDate,
  streakUnit,
} from './logic';

// 2026-08-08 is a Saturday.
const SATURDAY = new Date('2026-08-08T12:00:00');

function doneRecords(...dates: string[]): HabitRecord[] {
  return dates.map(date => ({ date, done: true }));
}

describe('isDueOnDate', () => {
  it('daily habits are due every day', () => {
    expect(isDueOnDate({ type: 'daily' }, '2026-08-08')).toBe(true);
  });

  it('weekdays habits are due only on selected days', () => {
    const freq: HabitFrequency = { type: 'weekdays', days: [1, 3, 5] };
    expect(isDueOnDate(freq, '2026-08-07')).toBe(true); // Friday
    expect(isDueOnDate(freq, '2026-08-08')).toBe(false); // Saturday
    expect(isDueOnDate(freq, '2026-08-09')).toBe(false); // Sunday
    expect(isDueOnDate(freq, '2026-08-10')).toBe(true); // Monday
  });

  it('weekdays habits with an empty day list are never due', () => {
    expect(isDueOnDate({ type: 'weekdays', days: [] }, '2026-08-10')).toBe(false);
  });

  it('weekly habits are always due', () => {
    expect(isDueOnDate({ type: 'weekly', timesPerWeek: 3 }, '2026-08-08')).toBe(true);
  });
});

describe('getDayOfWeekForDate', () => {
  it('maps dates to JS day-of-week (0=Sun..6=Sat)', () => {
    expect(getDayOfWeekForDate('2026-08-09')).toBe(0); // Sunday
    expect(getDayOfWeekForDate('2026-08-08')).toBe(6); // Saturday
    expect(getDayOfWeekForDate('2026-08-10')).toBe(1); // Monday
  });
});

describe('computeStreak — daily', () => {
  const daily = { type: 'daily' } as const;

  it('counts consecutive done days including today', () => {
    const records = doneRecords('2026-08-08', '2026-08-07', '2026-08-06', '2026-08-04');
    expect(computeStreak(daily, records, SATURDAY)).toBe(3);
  });

  it('is lenient when today is due but not done yet', () => {
    const records = doneRecords('2026-08-07', '2026-08-06');
    expect(computeStreak(daily, records, SATURDAY)).toBe(2);
  });

  it('breaks when yesterday was missed and today is not done', () => {
    const records = doneRecords('2026-08-05', '2026-08-04');
    expect(computeStreak(daily, records, SATURDAY)).toBe(0);
  });

  it('returns 0 with no records', () => {
    expect(computeStreak(daily, [], SATURDAY)).toBe(0);
  });
});

describe('computeStreak — weekdays', () => {
  // Mon / Wed / Fri
  const freq: HabitFrequency = { type: 'weekdays', days: [1, 3, 5] };

  it('skips non-due days while counting backwards', () => {
    // Sat 08-08 not due; Fri 08-07 done, Wed 08-05 done, Mon 08-03 missed → 2
    const records = doneRecords('2026-08-07', '2026-08-05');
    expect(computeStreak(freq, records, SATURDAY)).toBe(2);
  });

  it('counts through a full due week', () => {
    const records = doneRecords('2026-08-07', '2026-08-05', '2026-08-03', '2026-07-31');
    expect(computeStreak(freq, records, SATURDAY)).toBe(4);
  });

  it('is lenient on a due today that is not done yet', () => {
    // Viewed on Friday 08-07 (due, not done): streak counts Thu backwards → Wed done, Mon missed → 1
    const friday = new Date('2026-08-07T09:00:00');
    const records = doneRecords('2026-08-05');
    expect(computeStreak(freq, records, friday)).toBe(1);
  });
});

describe('computeStreak — weekly', () => {
  const weekly3 = { type: 'weekly', timesPerWeek: 3 } as const;

  it('counts consecutive weeks meeting the target, ignoring the current week', () => {
    // Current week (Mon 08-03 … Sun 08-09) has only 2 dones — ignored.
    // Previous week (07-27 … 08-02) has 3 dones, week before has 3 → streak 2.
    const records = doneRecords(
      '2026-08-05', '2026-08-07',
      '2026-07-28', '2026-07-30', '2026-08-01',
      '2026-07-21', '2026-07-23', '2026-07-25',
    );
    expect(computeStreak(weekly3, records, SATURDAY)).toBe(2);
  });

  it('stops at the first week below target', () => {
    const records = doneRecords(
      '2026-07-28', '2026-07-30', // previous week: only 2 of 3
      '2026-07-21', '2026-07-23', '2026-07-25',
    );
    expect(computeStreak(weekly3, records, SATURDAY)).toBe(0);
  });

  it('meeting the target exactly counts', () => {
    const records = doneRecords('2026-07-27', '2026-07-29', '2026-08-02');
    expect(computeStreak(weekly3, records, SATURDAY)).toBe(1);
  });
});

describe('computeWeekStatus', () => {
  it('returns done flags for the last 7 days, oldest first', () => {
    const records = doneRecords('2026-08-08', '2026-08-02');
    expect(computeWeekStatus(records, SATURDAY)).toEqual([
      true,  // 08-02
      false, // 08-03
      false, // 08-04
      false, // 08-05
      false, // 08-06
      false, // 08-07
      true,  // 08-08
    ]);
  });
});

describe('streakUnit', () => {
  it('is week for weekly habits and day otherwise', () => {
    expect(streakUnit({ type: 'weekly', timesPerWeek: 3 })).toBe('week');
    expect(streakUnit({ type: 'daily' })).toBe('day');
    expect(streakUnit({ type: 'weekdays', days: [1] })).toBe('day');
  });
});
