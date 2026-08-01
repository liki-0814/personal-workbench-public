import { beforeEach, describe, expect, it, vi } from 'vitest';

const fetchAllData = vi.fn();
const saveData = vi.fn();
const batchSaveData = vi.fn();

vi.mock('@/core/storage/apiClient', () => ({
  fetchAllData,
  saveData,
  batchSaveData,
}));

async function loadSyncEngine() {
  return import('@/core/storage/syncEngine');
}

describe('syncEngine', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    localStorage.clear();
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
});
