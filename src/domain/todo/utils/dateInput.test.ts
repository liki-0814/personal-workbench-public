import { describe, expect, it } from 'vitest';
import {
  addDateValue,
  calendarDays,
  isFourDigitDateValue,
  nextMondayValue,
  relativeDateValue,
  shiftMonthValue,
} from './dateInput';

describe('date input helpers', () => {
  it('only accepts complete dates with a four-digit year', () => {
    expect(isFourDigitDateValue('2026-08-04')).toBe(true);
    expect(isFourDigitDateValue('')).toBe(true);
    expect(isFourDigitDateValue('26-08-04')).toBe(false);
    expect(isFourDigitDateValue('12026-08-04')).toBe(false);
    expect(isFourDigitDateValue('2026-02-30')).toBe(false);
  });

  it('builds local relative dates across month boundaries', () => {
    expect(relativeDateValue(1, new Date(2026, 0, 31, 23, 30))).toBe('2026-02-01');
    expect(addDateValue('2026-08-04', 7)).toBe('2026-08-11');
  });

  it('moves between months without overflowing short months', () => {
    expect(shiftMonthValue('2026-01-31', 1)).toBe('2026-02-28');
    expect(shiftMonthValue('2024-03-31', -1)).toBe('2024-02-29');
  });

  it('builds a six-week calendar and finds next Monday', () => {
    const days = calendarDays('2026-08-04');
    expect(days).toHaveLength(42);
    expect(days[0]).toEqual({ value: '2026-07-26', day: 26, inMonth: false });
    expect(nextMondayValue(new Date(2026, 6, 28))).toBe('2026-08-03');
  });
});
