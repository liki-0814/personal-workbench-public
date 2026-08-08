import { beforeEach, describe, expect, it, vi } from 'vitest';

const fetchAllData = vi.fn();
const fetchData = vi.fn();
const saveData = vi.fn();
const batchSaveData = vi.fn();
const deleteData = vi.fn();

vi.mock('@/core/storage/apiClient', () => ({
  fetchAllData,
  fetchData,
  saveData,
  batchSaveData,
  deleteData,
}));

async function loadSyncEngine() {
  return import('@/core/storage/syncEngine');
}

describe('syncEngine', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    localStorage.clear();
    deleteData.mockResolvedValue(undefined);
    batchSaveData.mockResolvedValue(undefined);
  });

  it('persists item deletions to the backend as the filtered array', async () => {
    const { save, KEYS } = await loadSyncEngine();
    const todosAfterDelete = [
      { id: 'kept', title: 'Kept' },
    ];

    save(KEYS.TODOS, todosAfterDelete);

    expect(saveData).toHaveBeenCalledWith(KEYS.TODOS, todosAfterDelete);
  });

  it('does not resurrect id-based items that were deleted on the server', async () => {
    const { syncFromServer, KEYS } = await loadSyncEngine();
    localStorage.setItem('pwb_todos', JSON.stringify([
      { id: 'kept', title: 'Kept' },
      { id: 'deleted', title: 'Deleted' },
    ]));
    fetchAllData.mockResolvedValue({
      [KEYS.TODOS]: JSON.stringify([
        { id: 'kept', title: 'Kept' },
      ]),
    });

    await syncFromServer({ notify: false });

    expect(JSON.parse(localStorage.getItem('pwb_todos') || '[]')).toEqual([
      { id: 'kept', title: 'Kept' },
    ]);
    expect(batchSaveData).not.toHaveBeenCalled();
  });

  it('does not overwrite a local save while that key is still syncing', async () => {
    const { save, syncFromServer, KEYS } = await loadSyncEngine();
    saveData.mockReturnValue(new Promise(() => {}));

    save(KEYS.TODOS, [
      { id: 'kept', title: 'Kept' },
    ]);
    fetchAllData.mockResolvedValue({
      [KEYS.TODOS]: JSON.stringify([
        { id: 'kept', title: 'Kept' },
        { id: 'deleted', title: 'Deleted' },
      ]),
    });

    await syncFromServer({ notify: false });

    expect(JSON.parse(localStorage.getItem('pwb_todos') || '[]')).toEqual([
      { id: 'kept', title: 'Kept' },
    ]);
  });

  it('pulls dynamic prefixed keys and lets a newer local updatedAt win', async () => {
    const { syncFromServer } = await loadSyncEngine();
    localStorage.setItem('pwb_habit:habit-a', JSON.stringify({ id: 'habit-a', updatedAt: '2026-08-02T00:00:00.000Z' }));
    fetchAllData.mockResolvedValue({
      'habit:habit-a': JSON.stringify({ id: 'habit-a', updatedAt: '2026-08-01T00:00:00.000Z' }),
      'habit:habit-b': JSON.stringify({ id: 'habit-b', updatedAt: '2026-08-01T00:00:00.000Z' }),
    });

    await syncFromServer({ notify: false });

    expect(JSON.parse(localStorage.getItem('pwb_habit:habit-a')!).updatedAt).toBe('2026-08-02T00:00:00.000Z');
    expect(JSON.parse(localStorage.getItem('pwb_habit:habit-b')!).id).toBe('habit-b');
  });

  it('pushes local-only prefixed keys back to the server', async () => {
    const { syncFromServer } = await loadSyncEngine();
    localStorage.setItem('pwb_habit:habit-c', JSON.stringify({ id: 'habit-c', updatedAt: '2026-08-01T00:00:00.000Z' }));
    fetchAllData.mockResolvedValue({});

    await syncFromServer({ notify: false });

    expect(batchSaveData).toHaveBeenCalledWith([
      { key: 'habit:habit-c', value: { id: 'habit-c', updatedAt: '2026-08-01T00:00:00.000Z' } },
    ]);
  });

  it('incrementally syncs prefixed keys instead of skipping them', async () => {
    const { syncKeysFromServer } = await loadSyncEngine();
    fetchData.mockResolvedValue({ id: 'habit-d', updatedAt: '2026-08-01T00:00:00.000Z' });

    await syncKeysFromServer(['habit:habit-d'], { notify: false });

    expect(fetchData).toHaveBeenCalledWith('habit:habit-d');
    expect(JSON.parse(localStorage.getItem('pwb_habit:habit-d')!).id).toBe('habit-d');
  });

  it('removes a key locally and on the backend', async () => {
    const { remove } = await loadSyncEngine();
    localStorage.setItem('pwb_habit:habit-e', JSON.stringify({ id: 'habit-e' }));

    remove('habit:habit-e');

    expect(localStorage.getItem('pwb_habit:habit-e')).toBeNull();
    expect(deleteData).toHaveBeenCalledWith('habit:habit-e');
  });
});
