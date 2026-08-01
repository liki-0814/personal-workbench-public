export type HabitFrequency =
  | { type: 'daily' }
  | { type: 'weekly'; timesPerWeek: number }
  | { type: 'weekdays'; days: number[] }; // 0=Sun..6=Sat

export interface HabitRecord {
  date: string; // 'YYYY-MM-DD'
  done: boolean;
}

export interface HabitItem {
  id: string;
  title: string;
  emoji: string;
  frequency: HabitFrequency;
  records: HabitRecord[];
  createdAt: string;
  updatedAt: string;
  archived?: boolean;
}
