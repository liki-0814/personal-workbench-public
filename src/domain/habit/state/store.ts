import { useMemo, useSyncExternalStore } from 'react';
import type { HabitItem, HabitFrequency } from '../types';
import { load, save, remove, loadPrefixed, KEYS, STORAGE_SYNC_EVENT } from '@/core/storage';
import { now, formatDate } from '@/core/utils/date';
import { computeStreak, computeWeekStatus, isDueOnDate } from '../logic';

/**
 * Each habit is stored as its own small key ("habit:{id}") instead of one big
 * array, so a single check-in only rewrites/re-uploads that habit. Membership
 * is derived by scanning the key prefix; order is createdAt (matching the old
 * append order). Deletions propagate via the HABITS_DELETED tombstone list —
 * without it, another device's stale copy would resurrect a deleted habit on
 * the next sync.
 */
const HABIT_KEY_PREFIX = 'habit:';

function habitKey(id: string): string {
  return `${HABIT_KEY_PREFIX}${id}`;
}

interface HabitTombstone {
  id: string;
  updatedAt: string;
}

function generateHabitId(): string {
  return `habit-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
}

function loadTombstones(): HabitTombstone[] {
  return load<HabitTombstone[]>(KEYS.HABITS_DELETED, []);
}

/** One-time migration: split the legacy single "habits" array into per-habit keys. */
function migrateLegacyHabits(): void {
  const legacy = load<HabitItem[]>(KEYS.HABITS, []);
  if (legacy.length === 0) return;
  for (const habit of legacy) {
    if (habit && typeof habit.id === 'string') {
      save(habitKey(habit.id), habit);
    }
  }
  remove(KEYS.HABITS);
}

function byCreatedAt(a: HabitItem, b: HabitItem): number {
  return a.createdAt.localeCompare(b.createdAt) || a.id.localeCompare(b.id);
}

function scanHabits(): { habits: HabitItem[]; fingerprint: string } {
  const tombstones = loadTombstones();
  const tombstonedIds = new Set(tombstones.map(t => t.id));
  const entries = loadPrefixed<HabitItem>(HABIT_KEY_PREFIX);
  const habits: HabitItem[] = [];

  for (const entry of entries) {
    const habit = entry.value;
    if (!habit || typeof habit.id !== 'string') continue;
    if (tombstonedIds.has(habit.id)) {
      // Deleted on another device: drop the local copy and any server orphan.
      remove(entry.key);
      continue;
    }
    habits.push(habit);
  }

  habits.sort(byCreatedAt);
  const fingerprint =
    entries.map(e => `${e.key}=${e.raw}`).join('|') + '#' + JSON.stringify(tombstones);
  return { habits, fingerprint };
}

// One-time migration runs before the initial snapshot.
migrateLegacyHabits();

// Module-level shared store: multiple consumers (the habit panel, the
// workbench tab badge, …) must observe the same state. localStorage writes
// do not fire STORAGE_SYNC_EVENT within the tab that made them, so a plain
// per-hook useState copy would silently diverge between consumers.
const initial = scanHabits();
let allHabits: HabitItem[] = initial.habits;
let fingerprint: string = initial.fingerprint;
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): HabitItem[] {
  return allHabits;
}

function emit(): void {
  listeners.forEach(listener => listener());
}

function refreshFromStorage(): void {
  const next = scanHabits();
  fingerprint = next.fingerprint;
  allHabits = next.habits;
  emit();
}

// Reload when data changes elsewhere (backend sync pulls, other tabs).
// Our own writes already updated the snapshot; the fingerprint check keeps
// those notifications from causing redundant re-renders.
if (typeof window !== 'undefined') {
  window.addEventListener(STORAGE_SYNC_EVENT, () => {
    const next = scanHabits();
    if (next.fingerprint === fingerprint) return;
    fingerprint = next.fingerprint;
    allHabits = next.habits;
    emit();
  });
}

function persistHabit(habit: HabitItem): void {
  save(habitKey(habit.id), habit);
  refreshFromStorage();
}

export function addHabit(title: string, emoji: string, frequency: HabitFrequency): void {
  persistHabit({
    id: generateHabitId(),
    title,
    emoji,
    frequency,
    records: [],
    createdAt: now(),
    updatedAt: now(),
    archived: false,
  });
}

export function removeHabit(id: string): void {
  remove(habitKey(id));
  const tombstones = loadTombstones().filter(t => t.id !== id);
  tombstones.push({ id, updatedAt: now() });
  save(KEYS.HABITS_DELETED, tombstones);
  refreshFromStorage();
}

export function updateHabit(id: string, updates: Partial<Omit<HabitItem, 'id'>>): void {
  const habit = allHabits.find(h => h.id === id);
  if (!habit) return;
  persistHabit({ ...habit, ...updates, updatedAt: now() });
}

export function toggleToday(id: string): void {
  const habit = allHabits.find(h => h.id === id);
  if (!habit) return;
  const todayStr = formatDate(new Date());
  const existingIdx = habit.records.findIndex(r => r.date === todayStr);
  const records = existingIdx >= 0
    // Flip existing record
    ? habit.records.map((r, i) => (i === existingIdx ? { ...r, done: !r.done } : r))
    // Add new record as done
    : [...habit.records, { date: todayStr, done: true }];
  persistHabit({ ...habit, records, updatedAt: now() });
}

export function archiveHabit(id: string): void {
  updateHabit(id, { archived: true });
}

export function getStreak(id: string): number {
  const habit = allHabits.find(h => h.id === id);
  if (!habit) return 0;
  return computeStreak(habit.frequency, habit.records, new Date());
}

export function getWeekStatus(id: string): boolean[] {
  const habit = allHabits.find(h => h.id === id);
  if (!habit) return Array(7).fill(false);
  return computeWeekStatus(habit.records, new Date());
}

export function isTodayDue(habit: HabitItem): boolean {
  return isDueOnDate(habit.frequency, formatDate(new Date()));
}

export function useHabits() {
  const all = useSyncExternalStore(subscribe, getSnapshot);
  const habits = useMemo(() => all.filter(h => !h.archived), [all]);

  return {
    habits,
    allHabits: all,
    addHabit,
    removeHabit,
    updateHabit,
    toggleToday,
    archiveHabit,
    getStreak,
    getWeekStatus,
    isTodayDue,
  };
}
