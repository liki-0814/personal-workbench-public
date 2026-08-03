import { useEffect, useSyncExternalStore } from 'react';
import { apiFetch } from '@/core/utils';
import { STORAGE_SYNC_EVENT } from '@/core/storage/syncEngine';
import {
  normalizeProviderProtocol,
  replaceProvidersFromServer,
  type AiProvider,
  type ModelEntry,
  type ProviderProtocol,
} from './aiProviders';

export type ProviderKind = 'kimi-coding' | 'xai' | 'openai-codex' | 'qwen-token-plan-cn' | 'custom';
export type ProviderAuthMethod = 'oauth' | 'api_key';
export type ProviderAuthStatus = 'disconnected' | 'pending' | 'connected' | 'expired' | 'error';

export interface ProviderAuthView {
  method: ProviderAuthMethod;
  status: ProviderAuthStatus;
  accountLabel?: string;
  error?: string;
}

export interface ProviderView {
  id: string;
  kind: ProviderKind;
  name: string;
  auth: ProviderAuthView;
  models: ModelEntry[];
  defaultModel?: string;
  enabled: boolean;
  priority: number;
  builtin: boolean;
  baseUrl?: string;
  protocol?: ProviderProtocol;
  useProxy?: boolean;
}

export interface ProviderCatalogEntry {
  kind: Exclude<ProviderKind, 'custom'>;
  name: string;
  description: string;
  authMethod: ProviderAuthMethod;
  models: ModelEntry[];
  defaultModel?: string;
  connected?: boolean;
}

export interface ProviderAuthFlow {
  flowId: string;
  status: ProviderAuthStatus;
  method?: 'device' | 'browser';
  verificationUri?: string;
  userCode?: string;
  expiresAt?: number;
  pollIntervalMs?: number;
  error?: string;
}

export interface CustomProviderInput {
  name: string;
  baseUrl: string;
  protocol: Exclude<ProviderProtocol, 'openai' | 'anthropic'>;
  apiKey?: string;
  defaultModel: string;
  models: ModelEntry[];
  useProxy?: boolean;
}

interface ProviderSnapshot {
  providers: ProviderView[];
  catalog: ProviderCatalogEntry[];
  loading: boolean;
  loaded: boolean;
  error?: string;
}

const BUILTIN_CATALOG: ProviderCatalogEntry[] = [
  { kind: 'kimi-coding', name: 'Kimi Coding', description: '使用 Kimi Coding 订阅登录', authMethod: 'oauth', models: [] },
  { kind: 'xai', name: 'Grok / xAI', description: '使用 SuperGrok 或 X Premium 登录', authMethod: 'oauth', models: [] },
  { kind: 'openai-codex', name: 'OpenAI Codex', description: '使用 ChatGPT Plus / Pro 登录', authMethod: 'oauth', models: [] },
  { kind: 'qwen-token-plan-cn', name: 'Qwen Token Plan CN', description: '使用阿里云 Token Plan API Key', authMethod: 'api_key', models: [] },
];

let snapshot: ProviderSnapshot = { providers: [], catalog: BUILTIN_CATALOG, loading: false, loaded: false };
const listeners = new Set<() => void>();
let refreshPromise: Promise<void> | undefined;

function read<T>(value: Record<string, unknown>, camel: string, snake: string): T | undefined {
  return (value[camel] ?? value[snake]) as T | undefined;
}

function normalizeAuth(raw: unknown, fallback: ProviderAuthMethod): ProviderAuthView {
  const value = raw && typeof raw === 'object' ? raw as Record<string, unknown> : {};
  return {
    method: (value.method as ProviderAuthMethod | undefined) ?? fallback,
    status: (value.status as ProviderAuthStatus | undefined) ?? 'disconnected',
    accountLabel: read<string>(value, 'accountLabel', 'account_label'),
    error: typeof value.error === 'string' ? value.error : undefined,
  };
}

function normalizeProvider(raw: unknown, index: number): ProviderView {
  const value = raw && typeof raw === 'object' ? raw as Record<string, unknown> : {};
  const kind = (value.kind as ProviderKind | undefined) ?? 'custom';
  const builtin = (value.builtin as boolean | undefined) ?? kind !== 'custom';
  const endpoint = value.customEndpoint && typeof value.customEndpoint === 'object'
    ? value.customEndpoint as Record<string, unknown>
    : value.custom_endpoint && typeof value.custom_endpoint === 'object'
      ? value.custom_endpoint as Record<string, unknown>
      : value;
  const builtinProtocols: Partial<Record<ProviderKind, string>> = {
    'kimi-coding': 'anthropic_messages',
    xai: 'openai_chat',
    'openai-codex': 'openai_responses',
    'qwen-token-plan-cn': 'openai_chat',
  };
  const protocol = read<string>(endpoint, 'protocol', 'protocol') ?? builtinProtocols[kind];
  return {
    id: String(value.id ?? `provider-${index}`),
    kind,
    name: String(value.name ?? BUILTIN_CATALOG.find(item => item.kind === kind)?.name ?? 'Provider'),
    auth: normalizeAuth(value.auth, kind === 'qwen-token-plan-cn' || kind === 'custom' ? 'api_key' : 'oauth'),
    models: Array.isArray(value.models) ? value.models as ModelEntry[] : [],
    defaultModel: read<string>(value, 'defaultModel', 'default_model'),
    enabled: value.enabled !== false,
    priority: Number(value.priority ?? index),
    builtin,
    baseUrl: read<string>(endpoint, 'baseUrl', 'base_url'),
    protocol: protocol ? normalizeProviderProtocol(protocol) : undefined,
    useProxy: read<boolean>(endpoint, 'useProxy', 'use_proxy'),
  };
}

function normalizeCatalog(raw: unknown): ProviderCatalogEntry[] {
  raw = unwrapData(raw);
  const rows = Array.isArray(raw) ? raw : raw && typeof raw === 'object'
    ? ((raw as { providers?: unknown[]; catalog?: unknown[] }).providers ?? (raw as { catalog?: unknown[] }).catalog ?? [])
    : [];
  const remote = rows.map((item, index) => {
    const value = item && typeof item === 'object' ? item as Record<string, unknown> : {};
    const kind = value.kind as Exclude<ProviderKind, 'custom'>;
    return {
      kind,
      name: String(value.name ?? BUILTIN_CATALOG.find(entry => entry.kind === kind)?.name ?? `Provider ${index + 1}`),
      description: String(value.description ?? ''),
      authMethod: read<ProviderAuthMethod>(value, 'authMethod', 'auth_method') ?? 'oauth',
      models: Array.isArray(value.models) ? value.models as ModelEntry[] : [],
      defaultModel: read<string>(value, 'defaultModel', 'default_model'),
      connected: value.connected as boolean | undefined,
    } satisfies ProviderCatalogEntry;
  }).filter(item => BUILTIN_CATALOG.some(entry => entry.kind === item.kind));
  return BUILTIN_CATALOG.map(fallback => {
    const item = remote.find(candidate => candidate.kind === fallback.kind);
    return item ? { ...fallback, ...item, description: item.description || fallback.description } : fallback;
  });
}

function emit(next: ProviderSnapshot): void {
  snapshot = next;
  // Preserve the legacy config-backed model view until the dedicated endpoint
  // has returned successfully. A daemon from an older release may not expose
  // /api/providers yet, and an attempted refresh must not blank model pickers.
  if (next.loaded && !next.error) {
    const compatibilityProviders: AiProvider[] = next.providers.filter(provider => provider.enabled).map(provider => ({
      id: provider.id,
      name: provider.name,
      baseUrl: provider.baseUrl ?? '',
      apiKey: '',
      protocol: provider.protocol ?? 'openai_chat',
      models: provider.models,
      useProxy: provider.useProxy,
    }));
    replaceProvidersFromServer(compatibilityProviders);
  }
  listeners.forEach(listener => listener());
}

function unwrapList(raw: unknown): unknown[] {
  raw = unwrapData(raw);
  if (Array.isArray(raw)) return raw;
  if (raw && typeof raw === 'object') {
    const value = raw as { providers?: unknown[]; items?: unknown[] };
    return value.providers ?? value.items ?? [];
  }
  return [];
}

function unwrapData(raw: unknown): unknown {
  if (raw && typeof raw === 'object' && 'data' in raw) return (raw as { data: unknown }).data;
  return raw;
}

export function getProviderSnapshot(): ProviderSnapshot { return snapshot; }
export function subscribeProviders(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export async function refreshProviders(): Promise<void> {
  if (refreshPromise) return refreshPromise;
  emit({ ...snapshot, loading: true, error: undefined });
  refreshPromise = (async () => {
    try {
      const [providersRaw, catalogRaw] = await Promise.all([
        apiFetch('/api/providers'),
        apiFetch('/api/providers/catalog'),
      ]);
      const providers = unwrapList(providersRaw).map(normalizeProvider).sort((a, b) => a.priority - b.priority);
      emit({ providers, catalog: normalizeCatalog(catalogRaw), loading: false, loaded: true });
    } catch (error) {
      emit({ ...snapshot, loading: false, loaded: true, error: error instanceof Error ? error.message : 'AI 服务加载失败' });
      throw error;
    } finally {
      refreshPromise = undefined;
    }
  })();
  return refreshPromise;
}

export function useProviderStore(active = true): ProviderSnapshot {
  const state = useSyncExternalStore(subscribeProviders, getProviderSnapshot, getProviderSnapshot);
  useEffect(() => {
    if (active && !state.loaded && !state.loading) void refreshProviders().catch(() => undefined);
  }, [active, state.loaded, state.loading]);
  return state;
}

if (typeof window !== 'undefined') {
  window.addEventListener(STORAGE_SYNC_EVENT, event => {
    const reason = (event as CustomEvent<{ reason?: string }>).detail?.reason;
    if (reason !== 'provider-api' && snapshot.loaded && !snapshot.loading) {
      void refreshProviders().catch(() => undefined);
    }
  });
}

export async function createBuiltinProvider(kind: Exclude<ProviderKind, 'custom'>, apiKey?: string): Promise<void> {
  await apiFetch('/api/providers', {
    method: 'POST',
    body: JSON.stringify({ kind, ...(apiKey ? { apiKey } : {}) }),
  });
  await refreshProviders();
}

export async function saveCustomProvider(input: CustomProviderInput, id?: string): Promise<void> {
  const body = {
    kind: 'custom',
    name: input.name,
    baseUrl: input.baseUrl,
    protocol: normalizeProviderProtocol(input.protocol),
    apiKey: input.apiKey || undefined,
    defaultModel: input.defaultModel,
    models: input.models,
    useProxy: input.useProxy,
  };
  await apiFetch(id ? `/api/providers/${encodeURIComponent(id)}` : '/api/providers', {
    method: id ? 'PATCH' : 'POST',
    body: JSON.stringify(body),
  });
  await refreshProviders();
}

export async function updateProviderModels(
  id: string,
  models: ModelEntry[],
  defaultModel: string,
): Promise<void> {
  await apiFetch(`/api/providers/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body: JSON.stringify({ models, defaultModel }),
  });
  await refreshProviders();
}

export async function deleteProvider(id: string): Promise<void> {
  await apiFetch(`/api/providers/${encodeURIComponent(id)}`, { method: 'DELETE' });
  await refreshProviders();
}

export async function reorderProviderIds(ids: string[]): Promise<void> {
  await apiFetch('/api/providers/priorities', { method: 'PUT', body: JSON.stringify({ providerIds: ids }) });
  await refreshProviders();
}

export async function testProvider(id: string): Promise<void> {
  await apiFetch(`/api/providers/${encodeURIComponent(id)}/test`, { method: 'POST' });
}

export async function startProviderAuth(id: string, method: 'device' | 'browser'): Promise<ProviderAuthFlow> {
  const raw = await apiFetch<Record<string, unknown>>(`/api/providers/${encodeURIComponent(id)}/auth/start`, {
    method: 'POST', body: JSON.stringify({ method: method === 'device' ? 'device_code' : 'browser' }),
  });
  return normalizeAuthFlow(unwrapData(raw) as Record<string, unknown>);
}

export async function getProviderAuthStatus(id: string, flowId: string): Promise<ProviderAuthFlow> {
  const raw = await apiFetch<Record<string, unknown>>(`/api/providers/${encodeURIComponent(id)}/auth/status?flowId=${encodeURIComponent(flowId)}`);
  return normalizeAuthFlow(unwrapData(raw) as Record<string, unknown>);
}

export async function completeProviderAuth(id: string, flowId: string, value: string): Promise<ProviderAuthFlow> {
  const raw = await apiFetch<Record<string, unknown>>(`/api/providers/${encodeURIComponent(id)}/auth/complete`, {
    method: 'POST', body: JSON.stringify({ flowId, input: value }),
  });
  return normalizeAuthFlow(unwrapData(raw) as Record<string, unknown>);
}

export async function cancelProviderAuth(id: string, flowId: string): Promise<void> {
  await apiFetch(`/api/providers/${encodeURIComponent(id)}/auth/cancel`, { method: 'POST', body: JSON.stringify({ flowId }) });
}

export async function logoutProvider(id: string): Promise<void> {
  await apiFetch(`/api/providers/${encodeURIComponent(id)}/auth/logout`, { method: 'POST' });
  await refreshProviders();
}

function normalizeAuthFlow(value: Record<string, unknown>): ProviderAuthFlow {
  const rawState = String(value.status ?? value.state ?? 'pending');
  const status: ProviderAuthStatus = rawState === 'failed' || rawState === 'cancelled' ? 'error' : rawState as ProviderAuthStatus;
  const expires = read<number | string>(value, 'expiresAt', 'expires_at');
  return {
    flowId: String(read(value, 'flowId', 'flow_id') ?? ''),
    status,
    method: value.method === 'device_code' ? 'device' : value.method as 'device' | 'browser' | undefined,
    verificationUri: read(value, 'verificationUri', 'verification_uri'),
    userCode: read(value, 'userCode', 'user_code'),
    expiresAt: typeof expires === 'string' ? Date.parse(expires) : expires,
    pollIntervalMs: read(value, 'pollIntervalMs', 'poll_interval_ms'),
    error: typeof value.error === 'string' ? value.error : undefined,
  };
}
