import { fetchAllData, fetchData, saveData, batchSaveData, deleteData } from './apiClient';

const PREFIX = 'pwb_';
const pendingWriteVersions = new Map<string, number>();
let writeVersion = 0;

export const STORAGE_SYNC_EVENT = 'pwb:storage-synced';
export const STORAGE_SYNC_REQUEST_EVENT = 'pwb:storage-sync-request';

export const KEYS = {
  TODOS: 'todos',
  DAY_PLANS: 'day_plans',
  THEME: 'theme',
  POMODORO: 'pomodoro',
  POMODORO_SETTINGS: 'pomodoro_settings',
  CHAT_SESSIONS: 'chat_sessions',
  CHAT_PROJECTS: 'chat_projects',
  CHAT_FOLDERS: 'chat_folders',
  POMODORO_RECORDS: 'pomodoro_records',
  TODO_AUTO_SORT: 'todo_auto_sort',
  CLIPBOARD: 'clipboard',
  TOOLBOX_STATE: 'toolbox_state',
  AI_PROVIDERS: 'ai_providers',
  TASK_MODEL: 'task_model',
  GOALS: 'goals',
  HABITS: 'habits',
  HABITS_DELETED: 'habits_deleted',
  APP_CONFIG: 'app_config',
  MERMAID_ENABLED: 'mermaid_enabled',
} as const;

/** All data keys that should be synced. */
const SYNC_KEYS: string[] = Object.values(KEYS);

/** Keys whose array value should be replaced wholesale by server data
 *  (not union-merged). Use for arrays without an `id` field where union
 *  merge would leave duplicate-but-mutated entries. */
const SERVER_WINS_KEYS = new Set<string>([KEYS.AI_PROVIDERS, KEYS.APP_CONFIG]);

/** Prefixes for dynamic per-item key families (e.g. one key per habit:
 *  "habit:habit-123"). Each item syncs as its own small key so a single
 *  update does not rewrite or re-upload the whole collection. */
const SYNC_KEY_PREFIXES: string[] = ['habit:'];

/** Whether a key participates in server sync (static key or dynamic prefix family). */
function isSyncKey(key: string): boolean {
  return SYNC_KEYS.includes(key) || SYNC_KEY_PREFIXES.some(prefix => key.startsWith(prefix));
}

/** Sync read from localStorage cache. */
export function load<T>(key: string, defaultValue: T): T {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    if (!raw) return defaultValue;
    const parsed = JSON.parse(raw);
    if (Array.isArray(defaultValue) && !Array.isArray(parsed)) {
      console.warn(`Storage key "${key}" has invalid data type, resetting to default`);
      localStorage.removeItem(PREFIX + key);
      return defaultValue;
    }
    return parsed;
  } catch (error) {
    console.warn(`Failed to parse storage key "${key}", resetting to default:`, error);
    localStorage.removeItem(PREFIX + key);
    return defaultValue;
  }
}

/** Write to localStorage cache and async backend with retry. */
let localWriteNotifyScheduled = false;

function scheduleLocalWriteNotify(): void {
  if (localWriteNotifyScheduled) return;
  localWriteNotifyScheduled = true;
  queueMicrotask(() => {
    localWriteNotifyScheduled = false;
    notifyStorageSynced('local-write');
  });
}

export function save<T>(key: string, value: T): void {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch (e) {
    console.error(`Failed to serialize key "${key}" to localStorage:`, e instanceof Error ? e.message : e);
    return;
  }
  const version = ++writeVersion;
  pendingWriteVersions.set(key, version);
  syncWithRetry(key, value, version);
  scheduleLocalWriteNotify();
}

/** Remove a key from the localStorage cache and the backend. */
export function remove(key: string): void {
  localStorage.removeItem(PREFIX + key);
  deleteData(key).catch((error) => {
    console.warn(`Failed to delete key "${key}" on server:`, error);
  });
  scheduleLocalWriteNotify();
}

/** Read all entries stored under a dynamic key prefix (e.g. "habit:"). */
export function loadPrefixed<T>(keyPrefix: string): Array<{ key: string; value: T; raw: string }> {
  const fullPrefix = PREFIX + keyPrefix;
  const result: Array<{ key: string; value: T; raw: string }> = [];
  for (let i = 0; i < localStorage.length; i++) {
    const localKey = localStorage.key(i);
    if (!localKey || !localKey.startsWith(fullPrefix)) continue;
    const raw = localStorage.getItem(localKey);
    if (raw == null) continue;
    try {
      result.push({ key: localKey.slice(PREFIX.length), value: JSON.parse(raw) as T, raw });
    } catch {
      console.warn(`Storage key "${localKey}" has invalid JSON, skipping`);
    }
  }
  return result;
}

/** Retry sync with exponential backoff (max 3 attempts). */
async function syncWithRetry<T>(key: string, value: T, version: number, attempt = 1): Promise<void> {
  try {
    await saveData(key, value);
    if (pendingWriteVersions.get(key) === version) {
      pendingWriteVersions.delete(key);
    }
  } catch (error) {
    if (attempt < 3) {
      const delay = Math.min(1000 * Math.pow(2, attempt - 1), 5000);
      console.warn(`Sync "${key}" failed (attempt ${attempt}), retrying in ${delay}ms...`);
      setTimeout(() => syncWithRetry(key, value, version, attempt + 1), delay);
    } else {
      console.error(`Failed to sync key "${key}" to server after 3 attempts:`, error);
      // Release the in-flight lock so syncFromServer can resume pulling this key
      // once the backend recovers — otherwise this key is stuck for the session.
      if (pendingWriteVersions.get(key) === version) {
        pendingWriteVersions.delete(key);
      }
    }
  }
}

export interface SyncFromServerOptions {
  /** Emit a browser event when localStorage changed. */
  notify?: boolean;
  reason?: string;
}

function writeLocalRaw(localKey: string, value: string): boolean {
  if (localStorage.getItem(localKey) === value) return false;
  // WKWebView localStorage quota = 5MB per origin。某些大 key（如 chat_sessions）
  // 全量同步时可能超限抛 QuotaExceededError —— 单 key 失败不该让整个 syncFromServer 失败，
  // 否则前端拿不到 ai_providers 等小 key 数据，UI 看起来"未配置"。
  try {
    localStorage.setItem(localKey, value);
    return true;
  } catch (err) {
    const sizeKB = Math.round(value.length / 1024);
    console.warn(`[storage] localStorage.setItem("${localKey}", ${sizeKB}KB) failed:`, err);
    return false;
  }
}

function notifyStorageSynced(reason?: string): void {
  window.dispatchEvent(new CustomEvent(STORAGE_SYNC_EVENT, { detail: { reason } }));
}

export function requestSyncFromServer(reason = 'manual'): void {
  window.dispatchEvent(new CustomEvent(STORAGE_SYNC_REQUEST_EVENT, { detail: { reason } }));
}

/** Push all localStorage data to backend (browser wins). */
export async function syncToServer(): Promise<boolean> {
  const entries: Array<{ key: string; value: unknown }> = [];
  for (const key of SYNC_KEYS) {
    const raw = localStorage.getItem(PREFIX + key);
    if (!raw) continue;
    try {
      entries.push({ key, value: JSON.parse(raw) });
    } catch {
      /* skip invalid */
    }
  }
  for (const prefix of SYNC_KEY_PREFIXES) {
    for (const entry of loadPrefixed<unknown>(prefix)) {
      entries.push({ key: entry.key, value: entry.value });
    }
  }
  if (entries.length === 0) return true;
  try {
    await batchSaveData(entries);
    return true;
  } catch (error) {
    console.warn('[Sync] Failed to push to server:', error);
    return false;
  }
}

/**
 * Shared per-key merge logic used by both syncFromServer and syncKeysFromServer.
 * Handles: pending write skip → no local → SERVER_WINS → array mergeById → fallback server wins.
 */
function mergeAndWriteKey(key: string, serverParsed: unknown): boolean {
  const localKey = PREFIX + key;
  if (pendingWriteVersions.has(key)) return false;

  const serverRaw = JSON.stringify(serverParsed);
  const localRaw = localStorage.getItem(localKey);

  if (!localRaw) return writeLocalRaw(localKey, serverRaw);
  if (SERVER_WINS_KEYS.has(key)) return writeLocalRaw(localKey, serverRaw);

  try {
    const localParsed = JSON.parse(localRaw);
    if (Array.isArray(localParsed) && Array.isArray(serverParsed)) {
      const merged = mergeById(localParsed, serverParsed as Array<{ id: string; updatedAt?: string }>);
      return writeLocalRaw(localKey, JSON.stringify(merged));
    }
    if (isMergeableObject(localParsed) && isMergeableObject(serverParsed)) {
      // Single objects (e.g. one key per habit): newer updatedAt wins, server wins ties.
      const localTime = localParsed.updatedAt ? new Date(localParsed.updatedAt).getTime() : 0;
      const serverTime = serverParsed.updatedAt ? new Date(serverParsed.updatedAt).getTime() : 0;
      return writeLocalRaw(localKey, localTime > serverTime ? localRaw : serverRaw);
    }
    return writeLocalRaw(localKey, serverRaw);
  } catch {
    return writeLocalRaw(localKey, serverRaw);
  }
}

function isMergeableObject(value: unknown): value is { updatedAt?: string } {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Merge two arrays by id. The server snapshot is authoritative for deletions. */
function mergeById<T extends { id: string; updatedAt?: string }>(local: T[], server: T[]): T[] {
  const map = new Map<string, T>();
  for (const item of server) {
    map.set(item.id, item);
  }
  for (const item of local) {
    const existing = map.get(item.id);
    if (existing) {
      // Both have it: pick newer updatedAt, fallback to server
      const localTime = item.updatedAt ? new Date(item.updatedAt).getTime() : 0;
      const serverTime = existing.updatedAt ? new Date(existing.updatedAt).getTime() : 0;
      if (localTime > serverTime) {
        map.set(item.id, item);
      }
      // else keep server (already in map)
    }
  }
  return Array.from(map.values());
}

/** Incrementally sync only the specified keys from server.
 *  Uses per-key GET /api/data/:key instead of fetching the entire dataset.
 *  Same merge logic as syncFromServer but scoped to changed keys only.
 */
export async function syncKeysFromServer(keys: string[], options: SyncFromServerOptions = {}): Promise<boolean> {
  const { notify = true, reason } = options;
  // Filter to only known sync keys (static keys + dynamic prefix families)
  const validKeys = keys.filter(isSyncKey);
  if (validKeys.length === 0) return true;

  try {
    let changed = false;

    await Promise.all(validKeys.map(async (key) => {
      let serverParsed: unknown;
      try {
        serverParsed = await fetchData(key);
      } catch (err) {
        console.warn(`[Sync] Failed to fetch key "${key}":`, err);
        return;
      }

      if (serverParsed == null) return;

      if (mergeAndWriteKey(key, serverParsed)) changed = true;
    }));

    if (changed && notify) {
      notifyStorageSynced(reason);
    }
    return true;
  } catch (error) {
    console.warn('[Sync] Failed incremental sync, keys:', keys, error);
    return false;
  }
}

/** Pull data from backend, merge with local, and push merged result back.
 *  - Keys with local writes in flight are skipped so server data cannot clobber pending saves
 *  - SERVER_WINS_KEYS: server replaces local wholesale
 *  - Arrays: merged by id (all synced array types carry an `id`); server snapshot is
 *    authoritative for deletions, newer `updatedAt` wins on conflict
 *  - Non-array values: server wins
 *  Local-only keys are still pushed back when the server has no value for that key.
 */
export async function syncFromServer(options: SyncFromServerOptions = {}): Promise<boolean> {
  const { notify = true, reason } = options;
  try {
    const serverData = await fetchAllData();
    let changed = false;
    const toPush: Array<{ key: string; value: unknown }> = [];

    for (const [key, serverRaw] of Object.entries(serverData)) {
      if (!isSyncKey(key)) continue;
      if (serverRaw == null || serverRaw === '') continue;

      let serverParsed: unknown;
      try {
        serverParsed = JSON.parse(serverRaw);
      } catch {
        continue;
      }

      if (mergeAndWriteKey(key, serverParsed)) changed = true;
    }

    // Push local-only keys that server doesn't have
    for (const key of SYNC_KEYS) {
      if (serverData[key] == null || serverData[key] === '') {
        const localRaw = localStorage.getItem(PREFIX + key);
        if (localRaw) {
          try {
            toPush.push({ key, value: JSON.parse(localRaw) });
          } catch { /* skip */ }
        }
      }
    }
    for (const prefix of SYNC_KEY_PREFIXES) {
      for (const entry of loadPrefixed<unknown>(prefix)) {
        if (serverData[entry.key] == null || serverData[entry.key] === '') {
          toPush.push({ key: entry.key, value: entry.value });
        }
      }
    }

    // Push merged/local-only data back to server
    if (toPush.length > 0) {
      batchSaveData(toPush).catch((err) => {
        console.warn('[Sync] Failed to push merged data back to server:', err);
      });
    }

    if (changed && notify) {
      notifyStorageSynced(reason);
    }
    return true;
  } catch (error) {
    console.warn('Failed to sync from server, using local cache:', error);
    return false;
  }
}
