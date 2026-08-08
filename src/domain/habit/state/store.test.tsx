import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import type { HabitItem } from '../types';

const saveData = vi.fn();
const deleteData = vi.fn();

vi.mock('@/core/storage/apiClient', () => ({
  fetchAllData: vi.fn(),
  fetchData: vi.fn(),
  saveData,
  batchSaveData: vi.fn(),
  deleteData,
}));

type Store = typeof import('./store');
type HabitsState = ReturnType<Store['useHabits']>;

function habit(overrides: Partial<HabitItem>): HabitItem {
  return {
    id: 'habit-x',
    title: '阅读',
    emoji: '📚',
    frequency: { type: 'daily' },
    records: [],
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:00.000Z',
    archived: false,
    ...overrides,
  };
}

async function loadStore(): Promise<Store> {
  return import('./store');
}

describe('habit store', () => {
  let container: HTMLDivElement | null = null;
  let root: Root | null = null;
  let current: HabitsState | null = null;

  function Harness({ store }: { store: Store }) {
    current = store.useHabits();
    return null;
  }

  async function render(store: Store) {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    await act(async () => root!.render(<Harness store={store} />));
  }

  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    localStorage.clear();
    saveData.mockResolvedValue(undefined);
    deleteData.mockResolvedValue(undefined);
    current = null;
  });

  afterEach(async () => {
    if (root) await act(async () => root!.unmount());
    container?.remove();
    container = null;
    root = null;
  });

  it('migrates the legacy habits array into per-habit keys', async () => {
    localStorage.setItem('pwb_habits', JSON.stringify([
      habit({ id: 'habit-b', createdAt: '2026-01-02T00:00:00.000Z' }),
      habit({ id: 'habit-a', createdAt: '2026-01-01T00:00:00.000Z' }),
    ]));

    const store = await loadStore();
    await render(store);

    expect(localStorage.getItem('pwb_habits')).toBeNull();
    expect(JSON.parse(localStorage.getItem('pwb_habit:habit-a')!)).toMatchObject({ id: 'habit-a' });
    expect(JSON.parse(localStorage.getItem('pwb_habit:habit-b')!)).toMatchObject({ id: 'habit-b' });
    expect(deleteData).toHaveBeenCalledWith('habits');
    // Ordered by createdAt, matching the old append order
    expect(current!.habits.map(h => h.id)).toEqual(['habit-a', 'habit-b']);
  });

  it('writes adds and check-ins to that habit’s own key only', async () => {
    const store = await loadStore();
    await render(store);

    await act(async () => store.addHabit('冥想', '🧘', { type: 'daily' }));
    const id = current!.habits[0].id;
    expect(JSON.parse(localStorage.getItem(`pwb_habit:${id}`)!)).toMatchObject({ title: '冥想' });
    expect(saveData).toHaveBeenCalledWith(`habit:${id}`, expect.objectContaining({ title: '冥想' }));

    await act(async () => store.toggleToday(id));
    const stored = JSON.parse(localStorage.getItem(`pwb_habit:${id}`)!) as HabitItem;
    expect(stored.records).toHaveLength(1);
    expect(stored.records[0].done).toBe(true);
    // No aggregate collection key is written
    expect(localStorage.getItem('pwb_habits')).toBeNull();
  });

  it('removeHabit deletes the key and records a tombstone', async () => {
    localStorage.setItem('pwb_habit:habit-a', JSON.stringify(habit({ id: 'habit-a' })));
    const store = await loadStore();
    await render(store);

    await act(async () => store.removeHabit('habit-a'));

    expect(localStorage.getItem('pwb_habit:habit-a')).toBeNull();
    expect(deleteData).toHaveBeenCalledWith('habit:habit-a');
    const tombstones = JSON.parse(localStorage.getItem('pwb_habits_deleted')!) as Array<{ id: string }>;
    expect(tombstones.map(t => t.id)).toEqual(['habit-a']);
    expect(current!.habits).toEqual([]);
  });

  it('filters tombstoned habits pulled from the server and cleans the orphan key', async () => {
    localStorage.setItem('pwb_habit:habit-a', JSON.stringify(habit({ id: 'habit-a' })));
    localStorage.setItem('pwb_habits_deleted', JSON.stringify([
      { id: 'habit-a', updatedAt: '2026-08-01T00:00:00.000Z' },
    ]));

    const store = await loadStore();
    await render(store);

    expect(current!.habits).toEqual([]);
    expect(localStorage.getItem('pwb_habit:habit-a')).toBeNull();
    expect(deleteData).toHaveBeenCalledWith('habit:habit-a');
  });
});
