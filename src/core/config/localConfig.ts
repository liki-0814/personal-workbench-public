/**
 * Frontend client for the new `/api/local-config` endpoint (Track H3).
 *
 * Backend stores the nested schema in `~/.pwcli/config.json`. This module
 *   - GET-fetches the masked snapshot,
 *   - keeps an in-memory cache + emits storage-sync events on update,
 *   - PUT-patches deep into the JSON (server merges; we send only what changed),
 *   - listens for SSE `local_config` change broadcasts to refresh.
 *
 * Why a custom client (instead of saveData via syncEngine)?
 * - The data lives in a file outside SQLite/localStorage, so the merge engine
 *   doesn't apply. We want a thin REST client + opt-in cache.
 *
 * Sensitive field handling: the server returns `ai.mineruToken` as '******'
 * when present and non-empty. When PUTting, the same sentinel signals "untouched"
 * and the server preserves the existing real value.
 */

import { apiFetch } from '@/core/utils/apiFetch';
import { STORAGE_SYNC_EVENT } from '@/core/storage/syncEngine';
import type { MoaConfig } from './moa';

export const LOCAL_CONFIG_MASK = '******';
export const DEFAULT_FS_BASE = '~/';

/** Keep the effective default visible and persistable instead of representing it
 *  as an ambiguous empty field in Settings. */
export function normalizeFsBase(value?: string): string {
  return value?.trim() || DEFAULT_FS_BASE;
}

export interface LocalConfigCodeAgent {
  enabledBackends?: string[];
  /** Legacy per-backend defaults; no longer controls backend priority. */
  backend?: string;
  model?: string;
  contextWindow?: number;
  effort?: string;
  /** Free-form passthrough for forward compat (thinking/fast et al). */
  [key: string]: unknown;
}

export type DelegationExecutor = 'pwcli' | 'codex' | 'qoder' | 'kimi';
export type DelegationRole = 'researcher' | 'engineer' | 'reviewer' | 'analyst' | 'operator' | 'general';

export interface LocalConfigDelegationRole {
  label: string;
  routes: string[];
  names: string[];
}

export interface LocalConfigExecutorDefault {
  model: string;
  effort: string;
  permissionMode: string;
}

export interface LocalConfigDelegation {
  enabledExecutors?: DelegationExecutor[];
  cliPriority?: Exclude<DelegationExecutor, 'pwcli'>[];
  roles?: Partial<Record<DelegationRole, LocalConfigDelegationRole>>;
  executorDefaults?: Partial<Record<Exclude<DelegationExecutor, 'pwcli'>, LocalConfigExecutorDefault>>;
}

export interface LocalConfigAnySearch {
  /** Masked as '******' on GET when configured; empty uses anonymous quota. */
  apiKey?: string;
}

export interface LocalConfigGenImageModel {
  id?: string;
  name?: string;
  enabled?: boolean;
  protocol?: 'openai-images' | 'qwen-openai-images' | 'gemini-generate-content' | string;
  url?: string;
  apiKey?: string;
  model?: string;
  editUrl?: string;
  capabilities?: {
    generate?: boolean;
    reference?: boolean;
    edit?: boolean;
    preferredScenes?: VisualScene[];
  };
}

export type VisualScene = 'auto' | 'photo' | 'illustration' | 'poster' | 'infographic' | 'scientific' | 'product' | 'ui-mockup' | 'asset';

export interface LocalConfigGenImage {
  enabled?: boolean;
  defaultModel?: string;
  models?: LocalConfigGenImageModel[];
}

export interface LocalConfigSshServer {
  id: string;
  alias: string;
  host: string;
  port: number;
  user: string;
  auth:
    | { type: 'key'; path: string; passphrase?: string }
    | { type: 'password'; value: string };
  proxyJump?: string;
  description?: string;
  environment?: string;
  tags?: string[];
  location?: string;
  forwardAgent?: boolean;
  createdAt?: string;
  updatedAt?: string;
}

export interface LocalConfigTools {
  fsBase?: string;
  codeAgent?: LocalConfigCodeAgent;
  delegation?: LocalConfigDelegation;
  anySearch?: LocalConfigAnySearch;
  genImage?: LocalConfigGenImage;
  /** SSH configuration from config.json; secrets are masked on GET. */
  sshServers?: LocalConfigSshServer[];
}

export interface LocalConfigAi {
  moa?: MoaConfig;
  /** Masked as '******' on GET when configured; send same string on PUT to keep. */
  mineruToken?: string;
  responseLanguage?: ResponseLanguage;
}

export type ResponseLanguage = 'zh-CN' | 'en' | 'auto';

export interface LocalConfigMemory {
  injectionMode?: string;
  autoRetrieve?: boolean;
  maxResults?: number;
  maxDigestBytes?: number;
  dreamEnabled?: boolean;
  dreamProject?: string;
}

export interface LocalConfigFeatures {
  autoMemoryExtract?: boolean;
}

export interface LocalConfigServer {
  backendPort?: number;
  dataDir?: string;
  showHiddenFiles?: boolean;
  backupIntervalMs?: number;
}

export interface LocalConfigFrontend {
  backendUrl?: string;
  faviconServiceUrl?: string;
}

export interface LocalPermissionAllowRule {
  id: string;
  tool: string;
  arguments: string;
  cwd: string;
  expires_at?: string;
}

export type AgentPermissionMode = 'prompt' | 'risk' | 'full';

export interface LocalPermissionConfig {
  agent_mode?: AgentPermissionMode;
  allow_rules?: LocalPermissionAllowRule[];
}

export interface LocalConfig {
  schemaVersion?: number;
  ai?: LocalConfigAi;
  tools?: LocalConfigTools;
  memory?: LocalConfigMemory;
  features?: LocalConfigFeatures;
  server?: LocalConfigServer;
  frontend?: LocalConfigFrontend;
  permissions?: LocalPermissionConfig;
  /** Anything else pwcli/RuntimeConfig writes — preserved server-side. */
  [key: string]: unknown;
}

/** Deep partial mirror used for PUT patches. */
export type LocalConfigPatch = Partial<LocalConfig>;

interface ApiEnvelope<T> { success: boolean; data?: T; error?: string }

let _cache: LocalConfig = {};
let _initialized = false;
let _inflight: Promise<LocalConfig> | null = null;

/** Fetch + populate cache. Subsequent reads are sync via `getLocalConfig()`. */
export async function fetchLocalConfig(): Promise<LocalConfig> {
  if (_inflight) return _inflight;
  _inflight = (async () => {
    try {
      const env = await apiFetch<ApiEnvelope<LocalConfig>>('/api/local-config');
      if (env.success && env.data && typeof env.data === 'object') {
        _cache = env.data;
      }
      _initialized = true;
      notifySync();
      return _cache;
    } finally {
      _inflight = null;
    }
  })();
  return _inflight;
}

export function getLocalConfig(): LocalConfig {
  return _cache;
}

export function isLocalConfigReady(): boolean {
  return _initialized;
}

/**
 * PUT a partial patch. Server deep-merges with existing on disk.
 * `ai.mineruToken === '******'` is a no-op for that field on the server.
 */
export async function saveLocalConfig(patch: LocalConfigPatch): Promise<void> {
  await apiFetch('/api/local-config', {
    method: 'PUT',
    body: JSON.stringify(patch),
  });
  // Server will broadcast SSE; refresh local cache eagerly so the UI doesn't
  // wait for the round-trip (SSE adds ~hundreds of ms).
  await fetchLocalConfig();
  notifySync();
}

function notifySync(): void {
  if (typeof window === 'undefined') return;
  window.dispatchEvent(new CustomEvent(STORAGE_SYNC_EVENT, { detail: { reason: 'local-config' } }));
}

/** Hook into SSE: when key 'local_config' is broadcast, refresh + notify. */
export async function reloadLocalConfigFromSse(): Promise<void> {
  await fetchLocalConfig().catch(() => { /* ignore */ });
  notifySync();
}
