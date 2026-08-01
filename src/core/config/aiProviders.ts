import { save, KEYS, STORAGE_SYNC_EVENT } from '@/core/storage/syncEngine';

export interface ModelCapabilities {
  /** Accepts image inputs (multimodal vision). */
  vision?: boolean;
  /** Supports a togglable extended-thinking / reasoning mode. */
  thinking?: boolean;
}

export interface ModelEntry {
  id: string;
  name: string;
  /** 是否在选择器 dropdown 中显示。缺省/true=显示，false=隐藏。SettingsModal 编辑视图始终显示全部。 */
  enabled?: boolean;
  /** 单次输出 token 上限。缺省由 LLM 客户端按协议默认（Anthropic 4096，OpenAI 不传）。 */
  maxOutput?: number;
  /** 模型总上下文窗口。用于上下文占用展示与压缩阈值。 */
  contextWindow?: number;
  capabilities?: ModelCapabilities;
  /** Provider-specific top-level fields merged into every request for this model. */
  requestParams?: Record<string, unknown>;
  /** Provider-specific top-level fields merged only while extended thinking is enabled. */
  thinkingParams?: Record<string, unknown>;
  /** Cache-stable transcript loading for compatible models. */
  deferredToolsMode?: 'kimi';
}

export interface AiProvider {
  /** Stable server-assigned identity used to preserve masked credentials across reordering. */
  id?: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  protocol: 'openai' | 'anthropic';
  models: ModelEntry[];
}

export interface AiModelInfo {
  id: string;
  name: string;
  provider: 'openai' | 'anthropic';
  baseUrl: string;
  apiKey: string;
  providerIndex: number;
  /** Configured provider name (distinct from protocol in `provider`). */
  providerName?: string;
  enabled?: boolean;
  maxOutput?: number;
  contextWindow?: number;
  capabilities?: ModelCapabilities;
  requestParams?: Record<string, unknown>;
  thinkingParams?: Record<string, unknown>;
  deferredToolsMode?: 'kimi';
}

/* ---------- Runtime provider configuration ----------
 * Source of truth: `~/.pwcli/config.json` (mirrored to backend SQLite).
 * Frontend reads via syncFromServer → localStorage → here. */

const PROVIDERS_STORAGE_KEY = 'pwb_' + KEYS.AI_PROVIDERS;

function loadStoredProviders(): AiProvider[] {
  const raw = localStorage.getItem(PROVIDERS_STORAGE_KEY);
  if (!raw) return [];
  try {
    return JSON.parse(raw);
  } catch (err) {
    // Corrupted local cache — surface it instead of silently nuking the user's
    // provider list. Backend re-seed on next syncFromServer will restore.
    console.error('[ai] Corrupted providers in localStorage, falling back to empty:', err);
    return [];
  }
}

let _providers: AiProvider[] = loadStoredProviders();

function rebuildModels(): AiModelInfo[] {
  return _providers.flatMap((p, providerIndex) =>
    p.models.map(m => ({
      id: m.id,
      name: m.name,
      provider: p.protocol,
      baseUrl: p.baseUrl,
      apiKey: p.apiKey,
      providerIndex,
      providerName: p.name,
      enabled: m.enabled,
      maxOutput: m.maxOutput,
      contextWindow: m.contextWindow,
      capabilities: m.capabilities,
      requestParams: m.requestParams,
      thinkingParams: m.thinkingParams,
      deferredToolsMode: m.deferredToolsMode,
    }))
  );
}

let _models = rebuildModels();

function reloadProviders(): void {
  _providers = loadStoredProviders();
  _models = rebuildModels();
}

if (typeof window !== 'undefined') {
  window.addEventListener(STORAGE_SYNC_EVENT, reloadProviders);
}

export function getProviders(): AiProvider[] {
  return _providers;
}

/** 全部模型（含已隐藏的）。仅 SettingsModal 编辑视图、resolveModelId 按 ID 解析等需要看完整列表的场景使用。 */
export function getAllModels(): AiModelInfo[] {
  return _models;
}

/** 选择器视图：过滤掉 enabled === false 的。dropdown / 默认值挑选都用这个。 */
export function getModels(): AiModelInfo[] {
  return _models.filter(m => m.enabled !== false && !isImageModel(m.id));
}

export function setProviders(providers: AiProvider[]): void {
  _providers = providers;
  _models = rebuildModels();
  save(KEYS.AI_PROVIDERS, providers);
}

export function needsProxy(baseUrl: string): boolean {
  return baseUrl.includes('api.kimi.com');
}

/* ---------- Image model detection ---------- */

const GEMINI_IMAGE_MODEL_IDS = [
  'gemini-3.1-flash-image-preview',
  'gemini-2.5-flash-image',
];

export function isGeminiImageModel(modelId: string): boolean {
  const id = resolveModelId(modelId).toLowerCase();
  return GEMINI_IMAGE_MODEL_IDS.some(m => id === m || id.startsWith(m));
}

export function isQwenImageModel(modelId: string): boolean {
  return resolveModelId(modelId).toLowerCase().includes('qwen-image');
}

const IMAGE_MODEL_IDS = [
  'qwen-image-2.0-pro',
  'qwen-image-2.0',
  'gemini-3.1-flash-image-preview',
];

export function isImageModel(modelId: string): boolean {
  const id = resolveModelId(modelId).toLowerCase();
  return IMAGE_MODEL_IDS.includes(id) || isGeminiImageModel(id) || isQwenImageModel(id);
}

/* ---------- Capability detection (explicit config only) ----------
 * Reads `capabilities` declared on each model entry in config.json. NO whitelist /
 * substring inference — if a model isn't tagged, the feature is unavailable.
 * Add `"capabilities": {"vision": true, "thinking": true}` to the model JSON
 * in `providers[].models` to enable the corresponding UI affordances.
 */
function findModel(modelIdOrName: string): AiModelInfo | undefined {
  if (!modelIdOrName) return undefined;
  // 用全集查找（包含 enabled === false 的），让已选但被隐藏的模型仍能被 LLM 客户端解析。
  return _models.find(m => m.id === modelIdOrName || m.name === modelIdOrName);
}

export function isThinkingCapableModel(modelId: string): boolean {
  if (!modelId) return false;
  if (isImageModel(modelId)) return false;
  return !!findModel(modelId)?.capabilities?.thinking;
}

export function isVisionModel(modelId: string): boolean {
  if (!modelId) return false;
  // Image-generation models have their own request path; the input side has no
  // image upload, so they're not "vision" in the upload-image sense.
  if (isImageModel(modelId)) return false;
  return !!findModel(modelId)?.capabilities?.vision;
}

export function supportsKimiDeferredTools(provider: AiProvider): boolean {
  const name = provider.name.toLowerCase();
  const baseUrl = provider.baseUrl.toLowerCase();
  return name === 'kimi'
    || baseUrl.includes('api.kimi.com')
    || baseUrl.includes('moonshot');
}

export function resolveModelId(value: string): string {
  // 用全集解析：disabled 模型仍能正确识别 ID 用于 LLM 客户端。
  const byId = _models.find(m => m.id === value);
  if (byId) return byId.id;
  const byName = _models.find(m => m.name === value);
  if (byName) return byName.id;
  return value;
}

/* ---------- Request builders ---------- */

export function buildAiRequest(info: AiModelInfo): {
  url: string;
  headers: Record<string, string>;
} {
  if (info.provider === 'anthropic') {
    return {
      url: '/api/proxy/anthropic',
      headers: {
        'Content-Type': 'application/json',
        'X-Provider-Index': String(info.providerIndex),
      },
    };
  }

  return {
    url: '/api/proxy/openai',
    headers: {
      'Content-Type': 'application/json',
      'X-Provider-Index': String(info.providerIndex),
    },
  };
}

/* ---------- Feature model registry ---------- */

const DEFAULT_FEATURE_MODEL = 'qwen3.7-max';

function getDefaultFeatureModel(): string {
  const models = getModels();
  const found = models.find(m => m.id === DEFAULT_FEATURE_MODEL);
  return found?.id ?? (models[0]?.id ?? '');
}

export type FeatureModelKey = 'task';

interface FeatureModelDef {
  key: FeatureModelKey;
  label: string;
  description: string;
  storageKey: string;
  /** Legacy storage keys to fall back to when the primary key is unset. */
  legacyKeys?: string[];
}

export const FEATURE_MODELS: FeatureModelDef[] = [
  {
    key: 'task',
    label: '任务解析',
    description: '快速输入框 / 任务详情中将自然语言解析为结构化任务',
    storageKey: KEYS.TASK_MODEL,
  },
];

const STORAGE_PREFIX = 'pwb_';

function readStoredModel(storageKey: string): string | null {
  try {
    const raw = localStorage.getItem(STORAGE_PREFIX + storageKey);
    if (!raw) return null;
    const value = JSON.parse(raw);
    if (typeof value === 'string' && getModels().find(m => m.id === value || m.name === value)) {
      return value;
    }
  } catch {
    /* ignore */
  }
  return null;
}

function findFeature(key: FeatureModelKey): FeatureModelDef {
  const def = FEATURE_MODELS.find(f => f.key === key);
  if (!def) throw new Error(`Unknown feature model key: ${key}`);
  return def;
}

export function getFeatureModel(key: FeatureModelKey): string {
  const def = findFeature(key);
  const primary = readStoredModel(def.storageKey);
  if (primary) return primary;
  for (const legacy of def.legacyKeys ?? []) {
    const v = readStoredModel(legacy);
    if (v) return v;
  }
  return getDefaultFeatureModel();
}

export function setFeatureModel(key: FeatureModelKey, modelId: string): void {
  const def = findFeature(key);
  save(def.storageKey, modelId);
}

export function getModelInfo(id: string): AiModelInfo {
  // 用全集解析（已选模型若被 disabled 仍能拿到完整 info）；fallback 时用 enabled 视图的第一个。
  const byId = _models.find(m => m.id === id);
  if (byId) return byId;
  const byName = _models.find(m => m.name === id);
  if (byName) return byName;
  const enabled = getModels();
  if (enabled.length > 0) return enabled[0];
  if (_models.length > 0) return _models[0];
  return {
    id: '',
    name: '未配置',
    provider: 'openai',
    baseUrl: '',
    apiKey: '',
    providerIndex: 0,
  };
}
