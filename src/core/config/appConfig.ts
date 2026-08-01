import { save, KEYS, STORAGE_SYNC_EVENT } from '@/core/storage/syncEngine';

/** Workbench-wide runtime configuration sourced from `~/.pwcli/config.json`.
 *  Mirrors the bidirectional flow used for `ai_providers`:
 *  `config.json` → backend SQLite cache → frontend localStorage → here.
 *  Server replaces local wholesale via `SERVER_WINS_KEYS`.
 *
 *  这里只保留通过 SQLite 缓存分发给前端的字段；全部写回 config.json。
 *  （fsBase / mineruToken / codeAgent.{enabledBackends,model,contextWindow,effort} /
 *  Harness MoA）也走 `~/.pwcli/config.json`。 */
export interface AppConfig {
  /** UI 仅：思考模式开关。'true' | 'false'（空 = false） */
  codeAgentThinking?: string;
  /** UI 仅：快速模式开关。'true' | 'false'（空 = false） */
  codeAgentFast?: string;
  /** 文件浏览器是否显示 . 开头的隐藏文件/文件夹。'true' | 'false'（空 = false） */
  showHiddenFiles?: string;
}

const APP_CONFIG_STORAGE_KEY = 'pwb_' + KEYS.APP_CONFIG;

function loadStoredAppConfig(): AppConfig {
  const raw = localStorage.getItem(APP_CONFIG_STORAGE_KEY);
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as AppConfig;
    }
    return {};
  } catch (err) {
    // Corrupted local cache — fall back to empty so backend re-sync can restore.
    console.error('[appConfig] Corrupted app_config in localStorage, falling back to empty:', err);
    return {};
  }
}

let _appConfig: AppConfig = loadStoredAppConfig();

function reloadAppConfig(): void {
  _appConfig = loadStoredAppConfig();
}

if (typeof window !== 'undefined') {
  window.addEventListener(STORAGE_SYNC_EVENT, reloadAppConfig);
}

export function getAppConfig(): AppConfig {
  return _appConfig;
}

export function setAppConfig(next: AppConfig): void {
  _appConfig = next;
  save(KEYS.APP_CONFIG, next);
}
