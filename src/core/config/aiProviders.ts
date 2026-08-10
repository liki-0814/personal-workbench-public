import { save, KEYS, STORAGE_SYNC_EVENT } from '@/core/storage/syncEngine';

export interface ModelCapabilities {
  /** Accepts image inputs (multimodal vision). */
  vision?: boolean;
  /** Supports a togglable extended-thinking / reasoning mode. */
  thinking?: boolean;
  /** 生图模型：用于图像生成而非聊天，不出现在聊天模型选择器。 */
  image?: boolean;
  /** Provider 是否接受工具 input_schema 顶层的 oneOf/anyOf/allOf；缺省为支持。 */
  toolSchemaTopLevelCombinators?: boolean;
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
  /** Pi-style canonical level -> provider wire value mapping. Null hides an unsupported level. */
  thinkingLevelMap?: Record<string, unknown>;
  /** Cache-stable transcript loading for compatible models. */
  deferredToolsMode?: string;
}

export type ProviderProtocol =
  | 'openai_chat'
  | 'openai_responses'
  | 'anthropic_messages'
  | 'google_generative'
  | 'google_antigravity'
  // legacy aliases accepted when reading saved config
  | 'openai'
  | 'anthropic';

export const PROVIDER_PROTOCOL_OPTIONS: Array<{ value: ProviderProtocol; label: string; description: string }> = [
  { value: 'openai_chat', label: 'OpenAI Chat', description: 'Chat Completions / 兼容网关（含 Azure、Ollama）' },
  { value: 'openai_responses', label: 'OpenAI Responses', description: 'Responses API' },
  { value: 'anthropic_messages', label: 'Anthropic Messages', description: 'Claude Messages API' },
  { value: 'google_generative', label: 'Google Generative', description: 'Gemini generateContent 原生协议' },
];

export function normalizeProviderProtocol(protocol: string | undefined | null): ProviderProtocol {
  const value = (protocol || '').trim().toLowerCase().replace(/-/g, '_');
  switch (value) {
    case 'openai':
    case 'openai_chat':
    case 'openai_compatible':
      return 'openai_chat';
    case 'openai_responses':
    case 'openai_codex_responses':
    case 'openai_codex':
    case 'responses':
      return 'openai_responses';
    case 'anthropic':
    case 'anthropic_messages':
      return 'anthropic_messages';
    case 'google':
    case 'gemini':
    case 'google_generative':
    case 'generative_language':
      return 'google_generative';
    case 'google_antigravity':
      return 'google_antigravity';
    default:
      return 'openai_chat';
  }
}

export function protocolLabel(protocol: string | undefined | null): string {
  const normalized = normalizeProviderProtocol(protocol);
  return PROVIDER_PROTOCOL_OPTIONS.find(option => option.value === normalized)?.label || normalized;
}

export function protocolDescription(protocol: string | undefined | null): string {
  const normalized = normalizeProviderProtocol(protocol);
  return PROVIDER_PROTOCOL_OPTIONS.find(option => option.value === normalized)?.description || '';
}

/** Normalize legacy provider/model knobs into explicit fields. */
export function normalizeProviderConfig(provider: AiProvider): AiProvider {
  const protocol = normalizeProviderProtocol(provider.protocol);
  const baseUrl = (provider.baseUrl || '').trim();

  // Field-shape migration only. Never infer transport or vendor behavior from
  // provider names, base URLs, or model ids.
  const models = (provider.models || []).map(model => {
    const next = { ...model };
    if (next.deferredToolsMode && next.deferredToolsMode !== 'enabled') {
      // Historical values were vendor-ish strings; runtime only checks presence.
      next.deferredToolsMode = 'enabled';
    }
    return next;
  });

  return {
    ...provider,
    protocol,
    baseUrl,
    useProxy: provider.useProxy || undefined,
    models,
  };
}

export type ModelParamTemplate = {
  id: string;
  label: string;
  description: string;
  protocols: ProviderProtocol[];
  apply: (model: ModelEntry) => ModelEntry;
};

export const MODEL_PARAM_TEMPLATES: ModelParamTemplate[] = [
  {
    id: 'openai_top_p',
    label: '兼容网关 top_p',
    description: '写入 requestParams.top_p=0.95',
    protocols: ['openai_chat'],
    apply: model => ({
      ...model,
      requestParams: { ...(model.requestParams || {}), top_p: 0.95 },
    }),
  },
  {
    id: 'openai_responses_reasoning',
    label: 'Responses reasoning',
    description: 'thinkingParams.reasoning.effort=medium',
    protocols: ['openai_responses'],
    apply: model => ({
      ...model,
      capabilities: { ...(model.capabilities || {}), thinking: true },
      thinkingParams: {
        ...(model.thinkingParams || {}),
        reasoning: { effort: 'medium' },
      },
    }),
  },
  {
    id: 'anthropic_thinking_budget',
    label: 'Claude thinking budget',
    description: 'thinkingParams.budget_tokens=2048',
    protocols: ['anthropic_messages'],
    apply: model => ({
      ...model,
      capabilities: { ...(model.capabilities || {}), thinking: true },
      thinkingParams: {
        ...(model.thinkingParams || {}),
        budget_tokens: 2048,
      },
    }),
  },
  {
    id: 'gemini_include_thoughts',
    label: 'Gemini includeThoughts',
    description: 'thinkingParams.generationConfig.thinkingConfig',
    protocols: ['google_generative'],
    apply: model => ({
      ...model,
      capabilities: { ...(model.capabilities || {}), thinking: true },
      thinkingParams: {
        ...(model.thinkingParams || {}),
        generationConfig: {
          ...((model.thinkingParams?.generationConfig as Record<string, unknown> | undefined) || {}),
          thinkingConfig: { includeThoughts: true },
        },
      },
    }),
  },
];

export function templatesForProtocol(protocol: string | undefined | null): ModelParamTemplate[] {
  const normalized = normalizeProviderProtocol(protocol);
  return MODEL_PARAM_TEMPLATES.filter(template => template.protocols.includes(normalized));
}

/** Soft validation only: returns warnings, never blocks save. */
export function validateProviderConfig(provider: AiProvider): string[] {
  const protocol = normalizeProviderProtocol(provider.protocol);
  const warnings: string[] = [];

  for (const model of provider.models || []) {
    const req = model.requestParams || {};
    const think = model.thinkingParams || {};
    const reqKeys = Object.keys(req);
    const thinkKeys = Object.keys(think);

    if (protocol === 'openai_chat') {
      if (reqKeys.includes('thinking') || thinkKeys.includes('budget_tokens')) {
        warnings.push(`模型 ${model.id || model.name || '?'}: Anthropic 形态参数在 OpenAI Chat 下可能无效`);
      }
      if (thinkKeys.includes('reasoning') || reqKeys.includes('reasoning')) {
        warnings.push(`模型 ${model.id || model.name || '?'}: reasoning 更适合 OpenAI Responses 协议`);
      }
    }
    if (protocol === 'openai_responses') {
      if (thinkKeys.includes('budget_tokens') || reqKeys.includes('anthropic-version')) {
        warnings.push(`模型 ${model.id || model.name || '?'}: 含 Anthropic 专用字段，Responses 协议会忽略`);
      }
      if (thinkKeys.includes('enable_thinking')) {
        warnings.push(`模型 ${model.id || model.name || '?'}: enable_thinking 是 Chat Completions 习惯字段`);
      }
    }
    if (protocol === 'anthropic_messages') {
      if (thinkKeys.includes('enable_thinking') || thinkKeys.includes('reasoning_effort') || thinkKeys.includes('reasoning')) {
        warnings.push(`模型 ${model.id || model.name || '?'}: 含 OpenAI 思考字段，Anthropic 更常用 budget_tokens`);
      }
    }
    if (protocol === 'google_generative') {
      if (thinkKeys.includes('budget_tokens') || thinkKeys.includes('enable_thinking') || reqKeys.includes('anthropic-version')) {
        warnings.push(`模型 ${model.id || model.name || '?'}: 含非 Gemini 协议字段，可能被忽略`);
      }
    }
  }
  return warnings;
}

export interface AiProvider {
  /** Stable server-assigned identity used to preserve masked credentials across reordering. */
  id?: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  protocol: ProviderProtocol;
  models: ModelEntry[];
  /** Explicit transport flag: route via local daemon proxy. */
  useProxy?: boolean;
  /** @deprecated ignored by adapters; kept for old configs. */
  compatProfile?: string;
}

export interface AiModelInfo {
  id: string;
  name: string;
  provider: ProviderProtocol;
  baseUrl: string;
  apiKey: string;
  providerIndex: number;
  /** Stable server provider identity. Prefer this over positional indexes. */
  providerId?: string;
  /** Configured provider name (distinct from protocol in `provider`). */
  providerName?: string;
  useProxy?: boolean;
  enabled?: boolean;
  maxOutput?: number;
  contextWindow?: number;
  capabilities?: ModelCapabilities;
  requestParams?: Record<string, unknown>;
  thinkingParams?: Record<string, unknown>;
  thinkingLevelMap?: Record<string, unknown>;
  deferredToolsMode?: string;
}

/* ---------- Runtime provider configuration ----------
 * Source of truth: `~/.pwcli/config.json` (mirrored to backend SQLite).
 * Frontend reads via syncFromServer → localStorage → here. */

const PROVIDERS_STORAGE_KEY = 'pwb_' + KEYS.AI_PROVIDERS;

function loadStoredProviders(): AiProvider[] {
  const raw = localStorage.getItem(PROVIDERS_STORAGE_KEY);
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw) as AiProvider[];
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter(provider => provider && typeof provider === 'object')
      .map(provider => normalizeProviderConfig(provider));
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
    (p.models || []).map(m => ({
      id: m.id,
      name: m.name,
      provider: normalizeProviderProtocol(p.protocol),
      baseUrl: p.baseUrl,
      apiKey: p.apiKey,
      providerIndex,
      providerId: p.id,
      providerName: p.name,
      useProxy: p.useProxy,
      enabled: m.enabled,
      maxOutput: m.maxOutput,
      contextWindow: m.contextWindow,
      capabilities: m.capabilities,
      requestParams: m.requestParams,
      thinkingParams: m.thinkingParams,
      thinkingLevelMap: m.thinkingLevelMap,
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
  window.addEventListener(STORAGE_SYNC_EVENT, event => {
    const reason = (event as CustomEvent<{ reason?: string }>).detail?.reason;
    // ProviderStore has already replaced the in-memory projection and only
    // emits this event so hooks rerender. Reloading the intentionally removed
    // legacy localStorage value would immediately erase the server models.
    if (reason !== 'provider-api') reloadProviders();
  });
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
  _providers = providers.map(provider => normalizeProviderConfig(provider));
  _models = rebuildModels();
  save(KEYS.AI_PROVIDERS, _providers);
}

/** Update the compatibility model view from the server-backed ProviderStore.
 * Unlike setProviders this intentionally never writes provider configuration or
 * credentials to localStorage. */
export function replaceProvidersFromServer(providers: AiProvider[]): void {
  _providers = providers.map(provider => normalizeProviderConfig(provider));
  _models = rebuildModels();
  // The dedicated Provider API is authoritative and returns a redacted view.
  // Remove the legacy full-config cache after the first successful refresh so
  // API keys (including old unmasked values) cannot remain in browser storage.
  // eslint-disable-next-line no-restricted-syntax -- This removes a deprecated local-only credential cache; syncing a deletion would overwrite the authoritative Provider API.
  if (typeof localStorage !== 'undefined') localStorage.removeItem(PROVIDERS_STORAGE_KEY);
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent(STORAGE_SYNC_EVENT, { detail: { reason: 'provider-api' } }));
  }
}

export function needsProxy(provider: AiProvider): boolean {
  return provider.useProxy === true;
}

/* ---------- Image model detection ---------- */

const GEMINI_IMAGE_MODEL_IDS = [
  'gemini-3.1-flash-image-preview',
  'gemini-2.5-flash-image',
];

export function isGeminiImageModel(modelId: string): boolean {
  if (!modelId) return false;
  const id = resolveModelId(modelId).toLowerCase();
  return GEMINI_IMAGE_MODEL_IDS.some(m => id === m || id.startsWith(m));
}

export function isQwenImageModel(modelId: string): boolean {
  if (!modelId) return false;
  return resolveModelId(modelId).toLowerCase().includes('qwen-image');
}

const IMAGE_MODEL_IDS = [
  'qwen-image-2.0-pro',
  'qwen-image-2.0',
  'gemini-3.1-flash-image-preview',
];

export function isImageModel(modelId: string): boolean {
  if (!modelId) return false;
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
  const scoped = parseModelSelectionKey(modelIdOrName);
  if (scoped) {
    return _models.find(model => model.providerId === scoped.providerId && model.id === scoped.modelId);
  }
  // 用全集查找（包含 enabled === false 的），让已选但被隐藏的模型仍能被 LLM 客户端解析。
  const matches = _models.filter(m => m.id === modelIdOrName || m.name === modelIdOrName);
  // 旧会话只保存模型 ID：唯一时自动兼容，重名时禁止静默命中第一个 Provider。
  return matches.length === 1 ? matches[0] : undefined;
}

const MODEL_SELECTION_PREFIX = 'provider:';

export function modelSelectionKey(model: Pick<AiModelInfo, 'id' | 'providerId'>): string {
  return model.providerId
    ? `${MODEL_SELECTION_PREFIX}${encodeURIComponent(model.providerId)}/${encodeURIComponent(model.id)}`
    : model.id;
}

function parseModelSelectionKey(value: string): { providerId: string; modelId: string } | undefined {
  if (!value.startsWith(MODEL_SELECTION_PREFIX)) return undefined;
  const separator = value.indexOf('/', MODEL_SELECTION_PREFIX.length);
  if (separator < 0) return undefined;
  try {
    return {
      providerId: decodeURIComponent(value.slice(MODEL_SELECTION_PREFIX.length, separator)),
      modelId: decodeURIComponent(value.slice(separator + 1)),
    };
  } catch {
    return undefined;
  }
}

export function resolveModelSelection(value: string): string {
  if (!value) return '';
  const model = findModel(value);
  return model ? modelSelectionKey(model) : value;
}

export function isThinkingCapableModel(modelId: string): boolean {
  if (!modelId) return false;
  if (isImageModel(modelId)) return false;
  return !!findModel(modelId)?.capabilities?.thinking;
}

export const THINKING_LEVEL_ORDER = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra'] as const;
export type ThinkingLevel = typeof THINKING_LEVEL_ORDER[number];

export const RESPONSE_VERBOSITY_ORDER = ['low', 'medium', 'high'] as const;
export type ResponseVerbosity = typeof RESPONSE_VERBOSITY_ORDER[number];

export const RESPONSE_VERBOSITY_LABELS: Record<ResponseVerbosity, string> = {
  low: '简洁',
  medium: '适中',
  high: '详细',
};

export const THINKING_LEVEL_LABELS: Record<ThinkingLevel, string> = {
  off: '关闭',
  minimal: '极简',
  low: '低',
  medium: '中',
  high: '高',
  xhigh: '超高',
  max: '最大',
  ultra: '极致',
};

export function getSupportedThinkingLevels(modelId: string): ThinkingLevel[] {
  const model = findModel(modelId);
  return supportedThinkingLevelsForModel(model);
}

export function supportedThinkingLevelsForModel(
  model?: Pick<ModelEntry, 'capabilities' | 'thinkingLevelMap'>,
): ThinkingLevel[] {
  if (!model?.capabilities?.thinking) return ['off'];
  const declared = model.thinkingLevelMap;
  if (!declared || Object.keys(declared).length === 0) return ['off', 'medium'];
  return THINKING_LEVEL_ORDER.filter(level => level === 'off' || (
    Object.prototype.hasOwnProperty.call(declared, level) && declared[level] !== null
  ));
}

export function preferredThinkingLevel(levels: ThinkingLevel[]): ThinkingLevel {
  for (const level of ['high', 'medium', 'low', 'minimal'] as ThinkingLevel[]) {
    if (levels.includes(level)) return level;
  }
  return levels.find(level => level !== 'off') ?? 'off';
}

export function isVisionModel(modelId: string): boolean {
  if (!modelId) return false;
  // Image-generation models have their own request path; the input side has no
  // image upload, so they're not "vision" in the upload-image sense.
  if (isImageModel(modelId)) return false;
  return !!findModel(modelId)?.capabilities?.vision;
}

/** True when any model on this provider opts into deferred tool loading. */
export function supportsDeferredTools(provider: AiProvider): boolean {
  return provider.models.some(model => Boolean(model.deferredToolsMode));
}

/** @deprecated use supportsDeferredTools */
export function supportsKimiDeferredTools(provider: AiProvider): boolean {
  return supportsDeferredTools(provider);
}

export function resolveModelId(value: string): string {
  if (!value) return '';
  const scoped = parseModelSelectionKey(value);
  if (scoped) return scoped.modelId;
  // 用全集解析：disabled 模型仍能正确识别 ID 用于 LLM 客户端。
  const model = findModel(value);
  if (model) return model.id;
  return value;
}

/* ---------- Request builders ---------- */

export function buildAiRequest(info: AiModelInfo): {
  url: string;
  headers: Record<string, string>;
} {
  const providerHeader: Record<string, string> = info.providerId
    ? { 'X-Provider-Id': info.providerId }
    : { 'X-Provider-Index': String(info.providerIndex) };
  if (normalizeProviderProtocol(info.provider) === 'anthropic_messages') {
    return {
      url: '/api/proxy/anthropic',
      headers: {
        'Content-Type': 'application/json',
        ...providerHeader,
      },
    };
  }

  return {
    url: '/api/proxy/openai',
    headers: {
      'Content-Type': 'application/json',
      ...providerHeader,
    },
  };
}

/* ---------- Feature model registry ---------- */

const DEFAULT_FEATURE_MODEL = 'qwen3.7-max';

function getDefaultFeatureModel(): string {
  const models = getModels();
  const found = models.find(m => m.id === DEFAULT_FEATURE_MODEL);
  return found ? modelSelectionKey(found) : (models[0] ? modelSelectionKey(models[0]) : '');
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
    if (typeof value === 'string') {
      const resolved = resolveModelSelection(value);
      if (findModel(resolved)) return resolved;
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
  const selected = findModel(id);
  if (selected) return selected;
  if (id) {
    return {
      id: '',
      name: '请选择具体服务商',
      provider: 'openai_chat',
      baseUrl: '',
      apiKey: '',
      providerIndex: -1,
    };
  }
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
