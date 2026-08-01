import { useState, useEffect, useCallback, useRef } from 'react';
import Sortable from 'sortablejs';
import { X, Plus, Trash2, Settings, Server, CheckCircle, AlertCircle, GripVertical, ArrowUp, ArrowDown, Sparkles, Plug, RotateCcw, MonitorSmartphone, Eye, EyeOff, FolderOpen } from 'lucide-react';
import { MemorySettingsPanel } from '@/features/memory';
import type { AiProvider, FeatureModelKey, AppConfig, LocalConfig, LocalConfigGenImageModel, LocalConfigSshServer, ResponseLanguage, VisualScene } from '@/core/config';
import {
  getProviders, setProviders, useAiModels, FEATURE_MODELS, getFeatureModel, setFeatureModel,
  useAppConfig, setAppConfig,
  useLocalConfig, saveLocalConfig, LOCAL_CONFIG_TOKEN_MASK, normalizeFsBase,
  useMoaConfig, saveMoaConfig, getMoaConfig, getProviderModelOptions, validateMoaConfig,
  supportsKimiDeferredTools,
} from '@/core/config';
import HarnessMoaSettingsPanel from './HarnessMoaSettingsPanel';
import { getDefaultMaxOutput } from '@/core/llm/client';
import { getModelMeta } from '@/core/config/modelMetadata';
import { useStorageSync } from '@/core/storage';

import { SelectField, showToast, useModalDialog } from '@/shell';
import { getMermaidEnabled, setMermaidEnabled } from '@/shell/state/mermaidSettings';
import { apiFetch } from '@/core/utils';

interface Props {
  open: boolean;
  onClose: () => void;
}

interface DaemonStatus {
  healthy: boolean;
  version: string;
  httpAddress: string;
}

interface CodeAgentBackend {
  name: string;
  available: boolean;
  install_hint: string;
}

type Tab = 'ai' | 'models' | 'integrations' | 'memory' | 'ssh' | 'system' | 'about';

/** Local-config-backed fields rendered alongside the SQLite-cached AppConfig in the
 *  Integrations tab. They live in `~/.pwcli/config.json`; the form state groups them
 *  with AppConfig only to keep the settings form updates uniform. */
interface FormFields {
  fsBase?: string;
  codeAgentEnabledBackends?: string[];
  mineruToken?: string;
  anySearchApiKey?: string;
  genImageEnabled?: boolean;
  genImageDefaultModel?: string;
  genImageModels?: LocalConfigGenImageModel[];
}

interface SshServer extends LocalConfigSshServer {
  description: string;
  environment: string;
  tags: string[];
}

const normalizeSshServer = (server: LocalConfigSshServer): SshServer => ({
  ...server,
  port: server.port || 22,
  description: server.description ?? '',
  environment: server.environment ?? 'development',
  tags: server.tags ?? [],
});

const VISUAL_SCENES: Array<{ value: VisualScene; label: string }> = [
  { value: 'photo', label: '照片' },
  { value: 'illustration', label: '插画' },
  { value: 'poster', label: '海报' },
  { value: 'infographic', label: '信息图' },
  { value: 'scientific', label: '科研图' },
  { value: 'product', label: '产品图' },
  { value: 'ui-mockup', label: 'UI' },
  { value: 'asset', label: '素材' },
];

const CAPABILITY_OPTIONS = [
  { value: 'auto', label: '协议默认' },
  { value: 'true', label: '支持' },
  { value: 'false', label: '不支持' },
];

const CODE_AGENT_LABELS: Record<string, string> = {
  qoder: 'Qoder CLI',
  codex: 'Codex CLI',
  kimi: 'Kimi Code',
};

let genImageRowKeySequence = 0;

function createGenImageRowKey(): string {
  genImageRowKeySequence += 1;
  return `gen-image-row-${genImageRowKeySequence}`;
}

function capabilityValue(value: boolean | undefined): string {
  return value === undefined ? 'auto' : String(value);
}

function capabilityOverride(value: string): boolean | undefined {
  return value === 'auto' ? undefined : value === 'true';
}

function emptyProvider(): AiProvider {
  return {
    name: '',
    baseUrl: '',
    apiKey: '',
    protocol: 'openai',
    models: [],
  };
}

function JsonParamsEditor({
  label,
  value,
  placeholder,
  onChange,
}: {
  label: string;
  value?: Record<string, unknown>;
  placeholder: string;
  onChange: (value: Record<string, unknown> | undefined) => void;
}) {
  const [draft, setDraft] = useState(() => value ? JSON.stringify(value, null, 2) : '');
  const [error, setError] = useState('');

  const handleChange = (raw: string) => {
    setDraft(raw);
    if (!raw.trim()) {
      setError('');
      onChange(undefined);
      return;
    }
    try {
      const parsed: unknown = JSON.parse(raw);
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        setError('必须是 JSON 对象');
        return;
      }
      setError('');
      onChange(parsed as Record<string, unknown>);
    } catch {
      setError('JSON 格式不正确');
    }
  };

  return (
    <label className="block min-w-0">
      <span className="block text-[11px] text-gray-500 dark:text-gray-400 mb-1">{label}</span>
      <textarea
        value={draft}
        onChange={event => handleChange(event.target.value)}
        placeholder={placeholder}
        rows={3}
        spellCheck={false}
        aria-invalid={!!error}
        data-json-invalid={error ? 'true' : 'false'}
        className={`input-field w-full resize-y text-[11px] leading-4 font-mono dark:bg-white/5 dark:text-white ${
          error ? 'border-red-400 dark:border-red-500/70' : 'dark:border-white/10'
        }`}
      />
      {error && <span className="block mt-1 text-[10px] text-red-500">{error}</span>}
    </label>
  );
}
export default function SettingsModal({ open, onClose }: Props) {
  const [tab, setTab] = useState<Tab>('ai');
  const aiModels = useAiModels();
  const providerModelOptions = getProviderModelOptions();
  const moaConfig = useMoaConfig();
  const [localMoaConfig, setLocalMoaConfig] = useState(moaConfig);
  const moaConfigErrors = validateMoaConfig(localMoaConfig, providerModelOptions);
  const isMoaConfigDirty = JSON.stringify(localMoaConfig) !== JSON.stringify(moaConfig);
  const [providers, setLocalProviders] = useState<AiProvider[]>([]);
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const settingsDialogRef = useModalDialog({ open, onClose });
  const providerDialogRef = useModalDialog({
    open: open && editingIndex !== null,
    onClose: () => setEditingIndex(null),
  });
  const [editForm, setEditForm] = useState<AiProvider>(emptyProvider());
  const [daemonStatus, setDaemonStatus] = useState<DaemonStatus | null>(null);
  const [daemonCheckFailed, setDaemonCheckFailed] = useState(false);
  const [codeAgentBackends, setCodeAgentBackends] = useState<CodeAgentBackend[]>([]);
  const [codeAgentDetectionFailed, setCodeAgentDetectionFailed] = useState(false);
  const [featureModelMap, setFeatureModelMap] = useState<Record<FeatureModelKey, string>>(
    () => Object.fromEntries(FEATURE_MODELS.map(f => [f.key, getFeatureModel(f.key)])) as Record<FeatureModelKey, string>
  );
  const [mermaidEnabled, setMermaidEnabledState] = useState<boolean>(getMermaidEnabled);
  const [autoSaving, setAutoSaving] = useState(false);
  const [responseLanguageSaving, setResponseLanguageSaving] = useState(false);

  const localConfig = useLocalConfig();

  // SSH servers — config.json is the only source of truth.
  const [sshServers, setSshServers] = useState<SshServer[]>(
    () => (localConfig.tools?.sshServers ?? []).map(normalizeSshServer),
  );
  const [sshEditing, setSshEditing] = useState<SshServer | null>(null);
  const [sshShowPassword, setSshShowPassword] = useState(false);

  useEffect(() => {
    setSshServers((localConfig.tools?.sshServers ?? []).map(normalizeSshServer));
  }, [localConfig.tools?.sshServers]);

  const sshSave = useCallback(async (servers: SshServer[]): Promise<boolean> => {
    try {
      await saveLocalConfig({ tools: { sshServers: servers } });
      setSshServers(servers);
      return true;
    } catch (error) {
      showToast({
        message: error instanceof Error ? error.message : 'SSH 配置保存失败',
        type: 'error',
      });
      return false;
    }
  }, []);

  const sshNewServer = useCallback((): SshServer => ({
    id: `ssh-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
    alias: '',
    host: '',
    port: 22,
    user: 'root',
    auth: { type: 'password', value: '' },
    description: '',
    environment: 'development',
    tags: [],
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  }), []);

  // Integrations tab — config.json is canonical; SQLite only distributes updates to the frontend.
  // fsBase / mineruToken / Harness MoA 和 app_config 字段
  // 都由 ~/.pwcli/config.json 维护。
  const appConfig = useAppConfig();
  const lcTools = localConfig.tools;
  const lcAi = localConfig.ai;
  const delegationEnabledClis = lcTools?.delegation?.enabledExecutors
    ?.filter((executor): executor is 'codex' | 'qoder' | 'kimi' => executor !== 'pwcli');
  const delegationPwcliEnabled = lcTools?.delegation?.enabledExecutors?.includes('pwcli') ?? true;
  const delegationCliPriority = lcTools?.delegation?.cliPriority ?? [];
  const orderedDelegationClis = delegationEnabledClis && [
    ...delegationCliPriority.filter(executor => delegationEnabledClis.includes(executor)),
    ...delegationEnabledClis.filter(executor => !delegationCliPriority.includes(executor)),
  ];
  const legacyEnabledClis = lcTools?.codeAgent?.enabledBackends;
  const mergedAppConfig: AppConfig & FormFields = {
    // config.json → SQLite cache 来源
    showHiddenFiles: appConfig.showHiddenFiles ?? '',
    codeAgentThinking: appConfig.codeAgentThinking ?? '',
    codeAgentFast: appConfig.codeAgentFast ?? '',
    // local_config (~/.pwcli/config.json) 唯一真值
    fsBase: normalizeFsBase(lcTools?.fsBase),
    codeAgentEnabledBackends: orderedDelegationClis ?? (
      legacyEnabledClis && legacyEnabledClis.length > 0 ? legacyEnabledClis : undefined
    ),
    mineruToken: lcAi?.mineruToken ?? '',
    anySearchApiKey: lcTools?.anySearch?.apiKey ?? '',
    genImageEnabled: lcTools?.genImage?.enabled ?? false,
    genImageDefaultModel: lcTools?.genImage?.defaultModel ?? '',
    genImageModels: lcTools?.genImage?.models ?? [],
  };
  const [localAppConfig, setLocalAppConfig] = useState<AppConfig & FormFields>(mergedAppConfig);
  const [genImageRowKeys, setGenImageRowKeys] = useState(() =>
    (mergedAppConfig.genImageModels ?? []).map(createGenImageRowKey)
  );
  const updateGenImageModel = useCallback((index: number, patch: Partial<LocalConfigGenImageModel>) => {
    setLocalAppConfig(prev => ({
      ...prev,
      genImageModels: (prev.genImageModels ?? []).map((model, modelIndex) =>
        modelIndex === index ? { ...model, ...patch } : model
      ),
    }));
  }, []);
  const removeGenImageModel = useCallback((index: number) => {
    setGenImageRowKeys(previous => previous.filter((_, itemIndex) => itemIndex !== index));
    setLocalAppConfig(prev => ({
      ...prev,
      genImageModels: (prev.genImageModels ?? []).filter((_, itemIndex) => itemIndex !== index),
    }));
  }, []);
  const addGenImageModel = useCallback(() => {
    setGenImageRowKeys(previous => [...previous, createGenImageRowKey()]);
    setLocalAppConfig(prev => ({
      ...prev,
      genImageModels: [
        ...(prev.genImageModels ?? []),
        {
          id: `image-model-${(prev.genImageModels ?? []).length + 1}`,
          name: '',
          enabled: true,
          protocol: 'openai-images',
          url: '',
          apiKey: '',
          model: '',
        },
      ],
    }));
  }, []);
  const isAppConfigDirty =
    !(lcTools?.fsBase ?? '').trim() ||
    (localAppConfig.fsBase ?? '') !== (mergedAppConfig.fsBase ?? '') ||
    JSON.stringify(localAppConfig.codeAgentEnabledBackends ?? []) !==
      JSON.stringify(mergedAppConfig.codeAgentEnabledBackends ?? []) ||
    (localAppConfig.showHiddenFiles ?? '') !== (mergedAppConfig.showHiddenFiles ?? '') ||
    (localAppConfig.mineruToken ?? '') !== (mergedAppConfig.mineruToken ?? '') ||
    (localAppConfig.anySearchApiKey ?? '') !== (mergedAppConfig.anySearchApiKey ?? '') ||
    localAppConfig.genImageEnabled !== mergedAppConfig.genImageEnabled ||
    localAppConfig.genImageDefaultModel !== mergedAppConfig.genImageDefaultModel ||
    JSON.stringify(localAppConfig.genImageModels ?? []) !== JSON.stringify(mergedAppConfig.genImageModels ?? []);

  // Model list drag-to-reorder
  const modelListRef = useRef<HTMLDivElement>(null);
  const modelSortableRef = useRef<Sortable | null>(null);
  const modelRowKeysRef = useRef<string[]>([]);
  const modelRowKeyCounterRef = useRef(0);
  const providerListRef = useRef<HTMLDivElement>(null);
  const providerSortableRef = useRef<Sortable | null>(null);
  const providersRef = useRef(providers);
  providersRef.current = providers;

  const reorderProviders = useCallback((fromIndex: number, toIndex: number) => {
    const current = providersRef.current;
    if (
      fromIndex === toIndex ||
      fromIndex < 0 ||
      toIndex < 0 ||
      fromIndex >= current.length ||
      toIndex >= current.length
    ) {
      return;
    }
    const next = [...current];
    const [moved] = next.splice(fromIndex, 1);
    next.splice(toIndex, 0, moved);
    providersRef.current = next;
    setLocalProviders(next);
    setProviders(next);
  }, []);

  useEffect(() => {
    if (!open || tab !== 'ai' || !providerListRef.current) {
      providerSortableRef.current?.destroy();
      providerSortableRef.current = null;
      return;
    }
    providerSortableRef.current = new Sortable(providerListRef.current, {
      handle: '.provider-drag-handle',
      animation: 150,
      ghostClass: 'opacity-40',
      onEnd: (event) => {
        const { oldIndex, newIndex } = event;
        if (oldIndex == null || newIndex == null || oldIndex === newIndex) return;
        reorderProviders(oldIndex, newIndex);
        showToast({ message: 'Provider 优先级已更新', type: 'success' });
      },
    });
    return () => {
      providerSortableRef.current?.destroy();
      providerSortableRef.current = null;
    };
  }, [open, reorderProviders, tab]);

  useEffect(() => {
    if (!modelListRef.current || editingIndex === null) {
      modelSortableRef.current?.destroy();
      modelSortableRef.current = null;
      return;
    }
    modelSortableRef.current = new Sortable(modelListRef.current, {
      handle: '.model-drag-handle',
      animation: 150,
      ghostClass: 'opacity-40',
      onEnd: (evt) => {
        const { oldIndex, newIndex } = evt;
        if (oldIndex == null || newIndex == null || oldIndex === newIndex) return;
        const [movedKey] = modelRowKeysRef.current.splice(oldIndex, 1);
        modelRowKeysRef.current.splice(newIndex, 0, movedKey);
        setEditForm(prev => {
          const models = [...prev.models];
          const [moved] = models.splice(oldIndex, 1);
          models.splice(newIndex, 0, moved);
          return { ...prev, models };
        });
      },
    });
    return () => {
      modelSortableRef.current?.destroy();
      modelSortableRef.current = null;
    };
  }, [editingIndex]);

  const handleToggleMermaid = (next: boolean) => {
    setMermaidEnabledState(next);
    setMermaidEnabled(next);
    showToast({ message: next ? 'Mermaid 渲染已开启' : 'Mermaid 渲染已关闭，刷新页面生效', type: 'success' });
  };

  const handleFeatureModelChange = (key: FeatureModelKey, modelId: string) => {
    setFeatureModelMap(prev => ({ ...prev, [key]: modelId }));
    setFeatureModel(key, modelId);
    showToast({ message: '模型已更新', type: 'success' });
  };

  const handleResponseLanguageChange = async (responseLanguage: ResponseLanguage) => {
    if (responseLanguageSaving || responseLanguage === localConfig.ai?.responseLanguage) return;
    setResponseLanguageSaving(true);
    try {
      await saveLocalConfig({ ai: { responseLanguage } });
      const message = responseLanguage === 'zh-CN'
        ? '回复语言已切换为中文'
        : responseLanguage === 'en'
          ? 'Response language changed to English'
          : '回复语言已切换为跟随提问';
      showToast({ message, type: 'success' });
    } catch (error) {
      showToast({
        message: error instanceof Error ? `保存失败：${error.message}` : '保存失败',
        type: 'error',
      });
    } finally {
      setResponseLanguageSaving(false);
    }
  };

  const checkDaemon = useCallback(async () => {
    try {
      setDaemonCheckFailed(false);
      setDaemonStatus(await apiFetch<DaemonStatus>('/api/agent/daemon/status'));
    } catch {
      setDaemonStatus(null);
      setDaemonCheckFailed(true);
    }
  }, []);

  useEffect(() => {
    if (open) {
      setLocalProviders(getProviders());
      setFeatureModelMap(
        Object.fromEntries(FEATURE_MODELS.map(f => [f.key, getFeatureModel(f.key)])) as Record<FeatureModelKey, string>
      );
      setLocalAppConfig(mergedAppConfig);
      setGenImageRowKeys((mergedAppConfig.genImageModels ?? []).map(createGenImageRowKey));
      setLocalMoaConfig(moaConfig);
      checkDaemon();
    }
    // appConfig/localConfig intentionally excluded — we only re-seed on open, not on every SSE push,
    // so user edits aren't trampled mid-flight.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, checkDaemon]);

  // When SSE pushes new app_config / local-config and the user hasn't touched the form, refresh local view.
  useEffect(() => {
    if (open && !isAppConfigDirty) {
      setLocalAppConfig(mergedAppConfig);
      setGenImageRowKeys((mergedAppConfig.genImageModels ?? []).map(createGenImageRowKey));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [appConfig, localConfig, open]);

  useEffect(() => {
    if (open && !isMoaConfigDirty) {
      setLocalMoaConfig(moaConfig);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [moaConfig, open]);

  useEffect(() => {
    if (!open || tab !== 'integrations') return;
    setCodeAgentDetectionFailed(false);
    apiFetch<CodeAgentBackend[]>('/api/agent/backends?include_models=false')
      .then(setCodeAgentBackends)
      .catch(() => {
        setCodeAgentBackends([]);
        setCodeAgentDetectionFailed(true);
      });
  }, [open, tab]);

  const handleSaveMoaConfig = async () => {
    if (autoSaving) return;
    setAutoSaving(true);
    try {
      await saveMoaConfig(localMoaConfig);
      setLocalMoaConfig(getMoaConfig());
      showToast({ message: 'Harness MoA 配置已保存', type: 'success' });
    } catch (e) {
      showToast({
        message: e instanceof Error ? e.message : '保存失败，请检查后端是否运行',
        type: 'error',
      });
    } finally {
      setAutoSaving(false);
    }
  };

  const handleSaveAppConfig = async () => {
    // 字段拆分：
    //   local_config 路由 ← fsBase / mineruToken / anySearchApiKey
    //   app_config 缓存 ← showHiddenFiles，并保留旧版 codeAgent flags
    //
    // 空字符串 = 用户主动清空：
    //   - app_config 缓存端：保留为 ''，表示用户主动清空。
    //   - config.json 端：写空字符串 / 0；pwcli local_config 内部用 trim().is_empty()
    //     / `> 0` 判断，会自然 fallback 到 env / 默认值。
    //
    // 敏感字段（mineruToken）：值仍是 mask '******' → server 替回真值。空字符串 = 清空。
    const mineruRaw = (localAppConfig.mineruToken ?? '').trim();
    const anySearchRaw = (localAppConfig.anySearchApiKey ?? '').trim();
    const enabledDelegationClis = (localAppConfig.codeAgentEnabledBackends ?? [])
      .filter((executor): executor is 'codex' | 'qoder' | 'kimi' =>
        executor === 'codex' || executor === 'qoder' || executor === 'kimi');
    const genImageModels = (localAppConfig.genImageModels ?? []).map(model => ({
      ...model,
      id: (model.id ?? '').trim(),
      name: (model.name ?? '').trim(),
      protocol: (model.protocol ?? '').trim(),
      url: (model.url ?? '').trim(),
      editUrl: (model.editUrl ?? '').trim(),
      apiKey: (model.apiKey ?? '').trim(),
      model: (model.model ?? '').trim(),
    }));
    const localConfigPatch = {
      tools: {
        fsBase: normalizeFsBase(localAppConfig.fsBase),
        codeAgent: {
          enabledBackends: localAppConfig.codeAgentEnabledBackends ?? [],
        },
        delegation: {
          enabledExecutors: [...(delegationPwcliEnabled ? ['pwcli' as const] : []), ...enabledDelegationClis],
          cliPriority: enabledDelegationClis,
        },
        anySearch: {
          apiKey: anySearchRaw,
        },
        genImage: {
          enabled: localAppConfig.genImageEnabled ?? false,
          defaultModel: (localAppConfig.genImageDefaultModel ?? '').trim(),
          models: genImageModels,
        },
      },
      ai: {
        mineruToken: mineruRaw,
      },
    } satisfies Partial<LocalConfig>;

    const configBackedAppConfig: AppConfig = {
      showHiddenFiles: (localAppConfig.showHiddenFiles ?? '').trim(),
      codeAgentThinking: (localAppConfig.codeAgentThinking ?? '').trim(),
      codeAgentFast: (localAppConfig.codeAgentFast ?? '').trim(),
    };

    try {
      // Both routes write the same config.json, so keep them ordered to avoid
      // concurrent read-modify-write windows.
      await saveLocalConfig(localConfigPatch);
      setAppConfig(configBackedAppConfig);
    } catch (e) {
      showToast({
        message: e instanceof Error ? `保存失败：${e.message}` : '保存失败',
        type: 'error',
      });
      return;
    }

    // 保存后本地 mineruToken 与服务端将通过 SSE 推回的脱敏值对齐：
    //   - 刚保存了非空真值 / 仍是 mask → 服务端 GET 都会返 '******'
    //   - 刚清空（空字符串）            → 服务端 GET 返 ''
    const localMineru = mineruRaw === '' ? '' : LOCAL_CONFIG_TOKEN_MASK;
    const localAnySearch = anySearchRaw === '' ? '' : LOCAL_CONFIG_TOKEN_MASK;
    const localGenImageModels = genImageModels.map(model => ({
      ...model,
      apiKey: model.apiKey === '' ? '' : LOCAL_CONFIG_TOKEN_MASK,
    }));
    setLocalAppConfig({
      ...configBackedAppConfig,
      fsBase: normalizeFsBase(localAppConfig.fsBase),
      codeAgentEnabledBackends: localAppConfig.codeAgentEnabledBackends ?? [],
      mineruToken: localMineru,
      anySearchApiKey: localAnySearch,
      genImageEnabled: localAppConfig.genImageEnabled ?? false,
      genImageDefaultModel: (localAppConfig.genImageDefaultModel ?? '').trim(),
      genImageModels: localGenImageModels,
    });
    showToast({ message: '已保存。所有字段立即生效，无需重启', type: 'success' });
  };

  const handleResetFsBase = () => {
    setLocalAppConfig(prev => ({ ...prev, fsBase: normalizeFsBase() }));
  };

  const handleRevokePermissionRule = async (ruleId: string) => {
    const allowRules = (localConfig.permissions?.allow_rules ?? []).filter(rule => rule.id !== ruleId);
    try {
      await saveLocalConfig({ permissions: { allow_rules: allowRules } });
      showToast({ message: '已撤销权限规则', type: 'success' });
    } catch (error) {
      showToast({
        message: error instanceof Error ? `撤销失败：${error.message}` : '撤销失败',
        type: 'error',
      });
    }
  };




  // Refresh when another browser/tab pushes provider/model changes via SSE.
  useStorageSync(() => {
    if (!open) return;
    setLocalProviders(getProviders());
    setFeatureModelMap(
      Object.fromEntries(FEATURE_MODELS.map(f => [f.key, getFeatureModel(f.key)])) as Record<FeatureModelKey, string>
    );
  });

  const handleAddProvider = () => {
    modelRowKeysRef.current = [];
    setEditForm(emptyProvider());
    setEditingIndex(-1);
  };

  const handleEditProvider = (index: number) => {
    modelRowKeysRef.current = providers[index].models.map(
      () => `model-row-${modelRowKeyCounterRef.current++}`,
    );
    setEditForm({ ...providers[index] });
    setEditingIndex(index);
  };

  const handleDeleteProvider = (index: number) => {
    const next = providers.filter((_, i) => i !== index);
    providersRef.current = next;
    setLocalProviders(next);
    setProviders(next);
    showToast({ message: 'Provider 已删除', type: 'success' });
  };

  const handleMoveProvider = (index: number, direction: -1 | 1) => {
    reorderProviders(index, index + direction);
    showToast({ message: 'Provider 优先级已更新', type: 'success' });
  };

  const handleSaveEdit = () => {
    if (!editForm.name.trim() || !editForm.baseUrl.trim() || !editForm.apiKey.trim()) {
      showToast({ message: '请填写完整信息', type: 'error' });
      return;
    }
    if (providerDialogRef.current?.querySelector('[data-json-invalid="true"]')) {
      showToast({ message: '请先修正自定义请求参数的 JSON 格式', type: 'error' });
      return;
    }
    const next = [...providers];
    if (editingIndex === -1) {
      next.push(editForm);
    } else if (editingIndex !== null && editingIndex >= 0) {
      next[editingIndex] = editForm;
    }
    providersRef.current = next;
    setLocalProviders(next);
    setProviders(next);
    setEditingIndex(null);
    showToast({ message: 'Provider 已保存', type: 'success' });
  };

  // Model editor helpers
  const addModel = () => {
    modelRowKeysRef.current.push(`model-row-${modelRowKeyCounterRef.current++}`);
    setEditForm({
      ...editForm,
      models: [
        ...editForm.models,
        { id: '', name: '', enabled: true, capabilities: { vision: false, thinking: false } },
      ],
    });
  };

  const updateModel = (idx: number, field: 'id' | 'name', value: string) => {
    const next = [...editForm.models];
    next[idx] = { ...next[idx], [field]: value };
    setEditForm({ ...editForm, models: next });
  };

  const updateModelCapability = (idx: number, key: 'vision' | 'thinking', value: boolean) => {
    const next = [...editForm.models];
    const prevCaps = next[idx].capabilities || { vision: false, thinking: false };
    next[idx] = {
      ...next[idx],
      capabilities: { ...prevCaps, [key]: value },
    };
    setEditForm({ ...editForm, models: next });
  };

  const updateModelEnabled = (idx: number, value: boolean) => {
    const next = [...editForm.models];
    next[idx] = { ...next[idx], enabled: value };
    setEditForm({ ...editForm, models: next });
  };

  const updateModelDeferredTools = (idx: number, value: boolean) => {
    const next = [...editForm.models];
    if (value) {
      next[idx] = { ...next[idx], deferredToolsMode: 'kimi' };
    } else {
      const { deferredToolsMode: _omit, ...rest } = next[idx];
      next[idx] = rest;
    }
    setEditForm({ ...editForm, models: next });
  };

  const updateModelMaxOutput = (idx: number, value: string) => {
    const next = [...editForm.models];
    const trimmed = value.trim();
    if (trimmed === '') {
      const { maxOutput: _omit, ...rest } = next[idx];
      next[idx] = rest;
    } else {
      const num = Number(trimmed);
      if (!Number.isFinite(num) || num <= 0) return;
      next[idx] = { ...next[idx], maxOutput: Math.floor(num) };
    }
    setEditForm({ ...editForm, models: next });
  };

  const updateModelContextWindow = (idx: number, value: string) => {
    const next = [...editForm.models];
    const trimmed = value.trim();
    if (trimmed === '') {
      const { contextWindow: _omit, ...rest } = next[idx];
      next[idx] = rest;
    } else {
      const num = Number(trimmed);
      if (!Number.isFinite(num) || num <= 0) return;
      next[idx] = { ...next[idx], contextWindow: Math.floor(num) };
    }
    setEditForm({ ...editForm, models: next });
  };

  const updateModelParams = (
    idx: number,
    field: 'requestParams' | 'thinkingParams',
    value: Record<string, unknown> | undefined,
  ) => {
    const next = [...editForm.models];
    if (value) {
      next[idx] = { ...next[idx], [field]: value };
    } else {
      const { [field]: _omit, ...rest } = next[idx];
      next[idx] = rest;
    }
    setEditForm({ ...editForm, models: next });
  };

  const removeModel = (idx: number) => {
    modelRowKeysRef.current.splice(idx, 1);
    const next = editForm.models.filter((_, i) => i !== idx);
    setEditForm({ ...editForm, models: next });
  };

  if (!open) return null;

  return (
    <div className="settings-overlay fixed inset-0 z-[100] flex items-center justify-center">
      <div className="absolute inset-0 bg-black/40 backdrop-blur-sm dark:bg-black/60" onClick={onClose} />
      <div
        ref={settingsDialogRef}
        className="settings-dialog relative w-[720px] max-h-[85vh] flex flex-col rounded-2xl border shadow-2xl overflow-hidden bg-white dark:bg-[#1a1a1a]"
        role="dialog"
        aria-modal="true"
        aria-label="设置"
        tabIndex={-1}
        style={{ borderColor: 'rgba(0,0,0,0.08)' }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-gray-200 dark:border-white/10">
          <div className="flex items-center gap-2">
            <Settings size={18} className="text-gray-500 dark:text-gray-400" />
            <h2 className="text-base font-semibold text-gray-900 dark:text-white">设置</h2>
          </div>
          <button onClick={onClose} className="p-1.5 rounded-lg hover:bg-gray-100 dark:hover:bg-white/10 transition-colors" aria-label="关闭设置">
            <X size={18} className="text-gray-400 dark:text-gray-500" />
          </button>
        </div>

        {/* Tabs */}
        <div className="flex border-b border-gray-200 dark:border-white/10 px-6">
          {(['ai', 'models', 'integrations', 'memory', 'ssh', 'system', 'about'] as const).map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`settings-tab px-4 py-3 text-sm font-medium border-b-2 transition-colors ${
                tab === t
                  ? 'border-purple-500 text-purple-600 dark:text-purple-400'
                  : 'border-transparent text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'
              }`}
            >
              {t === 'ai' ? 'AI Provider'
                : t === 'models' ? 'AI 模型'
                : t === 'integrations' ? '工具与权限'
                : t === 'memory' ? '长期记忆'
                : t === 'ssh' ? 'SSH'
                : t === 'system' ? '系统状态'
                : '关于'}
            </button>
          ))}
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto p-6">
          {tab === 'ai' && (
            <div className="space-y-4">
              <div className="flex items-center justify-between">
                <div>
                  <p className="text-sm text-gray-600 dark:text-gray-300">
                    配置 AI Provider 以使用 AI 对话、任务拆解、笔记编辑等功能
                  </p>
                  <p className="text-xs text-gray-400 dark:text-gray-500 mt-0.5">
                    从上到下为 fallback 优先级；配置仅在当前设备生效，不会随 App 分发
                  </p>
                </div>
                <button
                  onClick={handleAddProvider}
                  className="flex items-center gap-1.5 px-3 py-2 text-xs font-medium rounded-lg bg-purple-500 text-white hover:bg-purple-600 transition-colors"
                >
                  <Plus size={14} />
                  添加 Provider
                </button>
              </div>

              {providers.length === 0 && (
                <div className="text-center py-10 text-gray-400 dark:text-gray-500 bg-gray-50 dark:bg-white/[0.03] rounded-xl">
                  <p className="text-sm">尚未配置任何 AI Provider</p>
                  <p className="text-xs mt-1">点击上方按钮添加，或从其他设备导入配置</p>
                </div>
              )}

              <div ref={providerListRef} className="space-y-3">
                {providers.map((p, i) => (
                  <div
                    key={`${p.name}-${p.baseUrl}`}
                    className="rounded-xl border border-gray-200 dark:border-white/10 p-4 bg-white dark:bg-white/[0.02] hover:border-gray-300 dark:hover:border-white/20 transition-colors"
                  >
                    <div className="flex items-center justify-between gap-3">
                      <div className="flex min-w-0 items-center gap-2.5">
                        <button
                          type="button"
                          className="provider-drag-handle -ml-1 shrink-0 cursor-grab touch-none rounded-md p-1 text-gray-300 hover:bg-gray-100 hover:text-gray-500 active:cursor-grabbing dark:text-gray-600 dark:hover:bg-white/10 dark:hover:text-gray-300"
                          aria-label={`拖动调整 ${p.name} 的优先级`}
                          title="拖动调整优先级"
                        >
                          <GripVertical size={16} />
                        </button>
                        <span className="shrink-0 text-[10px] font-medium tabular-nums text-purple-600 dark:text-purple-400">
                          {i + 1}
                        </span>
                        <span className="truncate text-sm font-medium text-gray-900 dark:text-white">{p.name}</span>
                        <span className="shrink-0 text-[10px] px-2 py-0.5 rounded-md bg-gray-100 dark:bg-white/10 text-gray-500 dark:text-gray-400 border border-gray-200 dark:border-white/10">
                          {p.protocol}
                        </span>
                        <span className="shrink-0 text-[10px] px-2 py-0.5 rounded-md bg-gray-100 dark:bg-white/10 text-gray-500 dark:text-gray-400 border border-gray-200 dark:border-white/10">
                          {p.models.length} 个模型
                        </span>
                      </div>
                      <div className="flex shrink-0 items-center gap-1">
                        <button
                          type="button"
                          onClick={() => handleMoveProvider(i, -1)}
                          disabled={i === 0}
                          className="p-1.5 rounded-md text-gray-400 hover:bg-gray-100 hover:text-gray-700 disabled:cursor-not-allowed disabled:opacity-25 dark:hover:bg-white/10 dark:hover:text-gray-200 transition-colors"
                          aria-label={`上移 ${p.name}`}
                          title="上移"
                        >
                          <ArrowUp size={14} />
                        </button>
                        <button
                          type="button"
                          onClick={() => handleMoveProvider(i, 1)}
                          disabled={i === providers.length - 1}
                          className="p-1.5 rounded-md text-gray-400 hover:bg-gray-100 hover:text-gray-700 disabled:cursor-not-allowed disabled:opacity-25 dark:hover:bg-white/10 dark:hover:text-gray-200 transition-colors"
                          aria-label={`下移 ${p.name}`}
                          title="下移"
                        >
                          <ArrowDown size={14} />
                        </button>
                        <button
                          onClick={() => handleEditProvider(i)}
                          className="px-2.5 py-1 rounded-md text-xs text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-white/10 transition-colors"
                        >
                          编辑
                        </button>
                        <button
                          onClick={() => handleDeleteProvider(i)}
                          className="p-1.5 rounded-md hover:bg-red-50 dark:hover:bg-red-500/10 text-red-500 transition-colors"
                          title="删除"
                          aria-label={`删除 ${p.name}`}
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </div>
                    <p className="ml-9 text-xs text-gray-400 dark:text-gray-500 font-mono truncate mt-1.5">{p.baseUrl}</p>
                  </div>
                ))}
              </div>
            </div>
          )}

          {tab === 'models' && (
            <div className="space-y-3">
              <div>
                <p className="text-sm text-gray-600 dark:text-gray-300">
                  为各 AI 固定流程链路单独指定模型
                </p>
                <p className="text-xs text-gray-400 dark:text-gray-500 mt-0.5">
                  AI 对话主输入框等保留独立模型选择能力，不受此配置影响
                </p>
              </div>

              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-4 bg-white dark:bg-white/[0.02]">
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <Sparkles size={14} className="text-purple-500 shrink-0" />
                      <span className="text-sm font-medium text-gray-900 dark:text-white">回复语言</span>
                    </div>
                    <p className="text-xs text-gray-400 dark:text-gray-500 mt-1 leading-relaxed">
                      适用于网页、TUI 和 CLI。当前对话明确指定语言时，仅覆盖该轮回复。
                    </p>
                  </div>
                  <SelectField
                    value={localConfig.ai?.responseLanguage ?? 'zh-CN'}
                    onValueChange={value => void handleResponseLanguageChange(value as ResponseLanguage)}
                    options={[
                      { value: 'zh-CN', label: '中文' },
                      { value: 'en', label: 'English' },
                      { value: 'auto', label: '跟随提问' },
                    ]}
                    disabled={responseLanguageSaving}
                    className="text-xs shrink-0 min-w-[140px]"
                    ariaLabel="选择 AI 回复语言"
                  />
                </div>
              </div>

              {aiModels.length === 0 ? (
                <div className="text-center py-10 text-amber-600 dark:text-amber-400 bg-amber-50 dark:bg-amber-500/10 rounded-xl border border-amber-200 dark:border-amber-500/20">
                  <p className="text-sm">尚未配置 AI Provider</p>
                  <p className="text-xs mt-1 text-amber-700 dark:text-amber-300/80">请先到「AI Provider」选项卡添加</p>
                </div>
              ) : (
                <div className="space-y-2">
                  {FEATURE_MODELS.map(f => (
                    <div
                      key={f.key}
                      className="rounded-xl border border-gray-200 dark:border-white/10 p-4 bg-white dark:bg-white/[0.02]"
                    >
                      <div className="flex items-start justify-between gap-4">
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-2">
                            <Sparkles size={14} className="text-purple-500 shrink-0" />
                            <span className="text-sm font-medium text-gray-900 dark:text-white">{f.label}</span>
                          </div>
                          <p className="text-xs text-gray-400 dark:text-gray-500 mt-1 leading-relaxed">{f.description}</p>
                        </div>
                        <SelectField
                          value={featureModelMap[f.key]}
                          onValueChange={value => handleFeatureModelChange(f.key, value)}
                          options={aiModels.map(model => ({ value: model.id, label: model.name }))}
                          className="text-xs shrink-0 min-w-[180px]"
                          ariaLabel={`为${f.label}选择模型`}
                        />
                      </div>
                    </div>
                  ))}
                </div>
              )}

              {providerModelOptions.length > 0 && (
                <HarnessMoaSettingsPanel
                  config={localMoaConfig}
                  modelOptions={providerModelOptions}
                  onChange={setLocalMoaConfig}
                  onSave={handleSaveMoaConfig}
                  isDirty={isMoaConfigDirty}
                  saving={autoSaving}
                  errors={moaConfigErrors}
                />
              )}
            </div>
          )}

          {tab === 'integrations' && (
            <div className="space-y-4">
              <div className="settings-note">
                <p className="text-xs leading-relaxed">
                  配置文件访问范围、外部工具和本机集成。修改写入 <code>~/.pwcli/config.json</code>，保存后立即生效。
                </p>
              </div>

              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
                <div className="flex items-center gap-2 mb-3">
                  <Plug size={15} className="text-gray-500 dark:text-gray-400" />
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">编码 CLI</h3>
                  <code className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-gray-100 dark:bg-white/10 text-gray-500 dark:text-gray-400">
                    tools.delegation.enabledExecutors
                  </code>
                </div>
                <div className="grid gap-2 sm:grid-cols-3">
                  {codeAgentBackends.map(backend => {
                    const availableNames = codeAgentBackends
                      .filter(item => item.available)
                      .map(item => item.name);
                    const configured = localAppConfig.codeAgentEnabledBackends;
                    const enabled = backend.available && (
                      configured === undefined
                        ? true
                        : configured.includes(backend.name)
                    );
                    return (
                      <button
                        key={backend.name}
                        type="button"
                        aria-pressed={enabled}
                        disabled={!backend.available || codeAgentDetectionFailed}
                        onClick={() => setLocalAppConfig(prev => {
                          const current = prev.codeAgentEnabledBackends === undefined
                            ? availableNames
                            : prev.codeAgentEnabledBackends;
                          const next = enabled
                            ? current.filter(name => name !== backend.name)
                            : [...current, backend.name];
                          return { ...prev, codeAgentEnabledBackends: next };
                        })}
                        className={`flex items-center justify-between rounded-lg border px-3 py-2 text-left text-sm transition ${
                          enabled
                            ? 'border-violet-400 bg-violet-50 text-violet-700 dark:border-violet-500/60 dark:bg-violet-500/10 dark:text-violet-200'
                            : 'border-gray-200 text-gray-500 dark:border-white/10 dark:text-gray-500'
                        } disabled:cursor-not-allowed disabled:opacity-50`}
                      >
                        <span>{CODE_AGENT_LABELS[backend.name] ?? backend.name}</span>
                        {enabled && <CheckCircle size={14} />}
                      </button>
                    );
                  })}
                </div>
                <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-2 leading-relaxed">
                  {codeAgentDetectionFailed
                    ? '无法从 daemon 获取 CLI 探测结果。'
                    : codeAgentBackends.length === 0
                      ? '正在检测本机可用的编码 CLI…'
                      : `只会使用这里启用且本机可用的 CLI；未指定 CLI 时由 pwcli 按任务动态选择。`}
                </p>
              </div>

              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
                <div className="flex items-center gap-2 mb-3">
                  <Plug size={15} className="text-gray-500 dark:text-gray-400" />
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">已保存的权限规则</h3>
                </div>
                {(localConfig.permissions?.allow_rules ?? []).length === 0 ? (
                  <p className="text-xs text-gray-400 dark:text-gray-500">暂无规则。“始终允许此操作”只会保存工具、完整参数和工作目录完全一致的授权。</p>
                ) : (
                  <div className="space-y-2">
                    {(localConfig.permissions?.allow_rules ?? []).map(rule => (
                      <div key={rule.id} className="flex items-start gap-3 rounded-lg border border-gray-100 dark:border-white/10 p-3">
                        <div className="min-w-0 flex-1">
                          <div className="text-xs font-medium text-gray-800 dark:text-gray-200">{rule.tool}</div>
                          <div className="mt-1 truncate text-[11px] font-mono text-gray-500 dark:text-gray-400" title={rule.arguments}>{rule.arguments}</div>
                          <div className="mt-1 truncate text-[10px] text-gray-400 dark:text-gray-500" title={rule.cwd}>{rule.cwd}{rule.expires_at ? ` · ${new Date(rule.expires_at).toLocaleDateString()} 到期` : ''}</div>
                        </div>
                        <button
                          type="button"
                          onClick={() => void handleRevokePermissionRule(rule.id)}
                          className="shrink-0 rounded-md p-1.5 text-red-500 transition-colors hover:bg-red-50 dark:hover:bg-red-500/10"
                          title="撤销此规则"
                        >
                          <Trash2 size={13} />
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </div>

              {/* tools.fsBase */}
              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
                <div className="flex items-center gap-2 mb-3">
                  <FolderOpen size={15} className="text-gray-500 dark:text-gray-400" />
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">允许访问的根目录</h3>
                  <code className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-gray-100 dark:bg-white/10 text-gray-500 dark:text-gray-400">
                    tools.fsBase
                  </code>
                </div>
                <div className="flex items-center gap-2">
                  <input
                    type="text"
                    value={localAppConfig.fsBase ?? ''}
                    onChange={e => setLocalAppConfig(prev => ({ ...prev, fsBase: e.target.value }))}
                    placeholder="~/"
                    aria-label="允许访问的根目录"
                    className="input-field flex-1 text-sm font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                  />
                  <button
                    onClick={handleResetFsBase}
                    title="恢复为当前用户的主目录 ~/"
                    className="flex items-center gap-1 px-2.5 py-2 text-xs rounded-lg border border-gray-200 dark:border-white/10 hover:bg-gray-100 dark:hover:bg-white/10 text-gray-600 dark:text-gray-300 transition-colors shrink-0"
                  >
                    <RotateCcw size={12} />
                    恢复 ~/
                  </button>
                </div>
                <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-2 leading-relaxed">
                  AI、文件选择器和 Studio 只能访问该目录及其子目录。保存后立即生效，无需重启；默认 <code className="px-1 rounded bg-gray-100 dark:bg-white/10 font-mono">~/</code>。
                </p>
              </div>

              {/* MinerU 文档解析 */}
              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
                <div className="flex items-center gap-2 mb-3">
                  <Plug size={15} className="text-gray-500 dark:text-gray-400" />
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">MinerU 文档解析</h3>
                </div>
                <input
                  type="password"
                  value={localAppConfig.mineruToken ?? ''}
                  onChange={e => setLocalAppConfig(prev => ({ ...prev, mineruToken: e.target.value }))}
                  placeholder={
                    (localConfig.ai?.mineruToken ?? '') === LOCAL_CONFIG_TOKEN_MASK
                      ? '******'
                      : '可选 API Token（远程 Precision 模式）'
                  }
                  autoComplete="off"
                  className="input-field w-full text-sm font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                />
                <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-2 leading-relaxed">
                  可选 API Token，配置后启用远程 Precision 模式。获取：
                  <a
                    href="https://mineru.net/apiManage/apiKeys"
                    target="_blank"
                    rel="noopener noreferrer"
                    className="ml-0.5 text-purple-600 dark:text-purple-400 hover:underline"
                  >
                    mineru.net/apiManage
                  </a>
                  <br />
                  pwcli 仅使用远程 MinerU API，不探测、不安装也不调用本地 MinerU。
                </p>
              </div>

              {/* tools.genImage */}
              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
                <div className="flex items-center gap-2 mb-3">
                  <Sparkles size={15} className="text-purple-500" />
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">内置生图工具</h3>
                  <code className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-gray-100 dark:bg-white/10 text-gray-500 dark:text-gray-400">
                    tools.genImage
                  </code>
                </div>
                <div className="grid grid-cols-2 gap-3 mb-4">
                  <SelectField
                    value={localAppConfig.genImageEnabled ? 'true' : 'false'}
                    onValueChange={value => setLocalAppConfig(prev => ({ ...prev, genImageEnabled: value === 'true' }))}
                    options={[{ value: 'true', label: '启用' }, { value: 'false', label: '停用' }]}
                    className="w-full text-sm"
                    ariaLabel="生图工具开关"
                  />
                  <SelectField
                    value={localAppConfig.genImageDefaultModel ?? ''}
                    onValueChange={value => setLocalAppConfig(prev => ({ ...prev, genImageDefaultModel: value }))}
                    options={(localAppConfig.genImageModels ?? []).map(model => ({
                      value: model.id ?? '',
                      label: model.name || model.model || model.id || '未命名模型',
                    }))}
                    className="w-full text-sm"
                    ariaLabel="默认生图模型"
                  />
                </div>
                {(localAppConfig.genImageModels ?? []).map((model, index) => (
                  <div key={genImageRowKeys[index]} className="mt-3 rounded-lg border border-gray-100 dark:border-white/10 p-3">
                    <div className="mb-2 flex items-center justify-between">
                      <span className="text-xs font-medium text-gray-700 dark:text-gray-200">{model.name || model.model || '生图模型'}</span>
                      <div className="flex items-center gap-2">
                        <button
                          type="button"
                          onClick={() => updateGenImageModel(index, { enabled: !model.enabled })}
                          className={`rounded-full px-2.5 py-1 text-[10px] ${model.enabled ? 'bg-purple-100 text-purple-700 dark:bg-purple-500/20 dark:text-purple-300' : 'bg-gray-100 text-gray-500 dark:bg-white/10 dark:text-gray-400'}`}
                        >
                          {model.enabled ? '已启用' : '未启用'}
                        </button>
                        <button
                          type="button"
                          onClick={() => removeGenImageModel(index)}
                          className="text-gray-400 hover:text-red-500"
                          aria-label="删除生图模型"
                        >
                          <Trash2 size={13} />
                        </button>
                      </div>
                    </div>
                    <div className="grid gap-2">
                      <input
                        type="text"
                        value={model.id ?? ''}
                        onChange={event => updateGenImageModel(index, { id: event.target.value })}
                        placeholder="本地模型 ID（唯一）"
                        className="input-field w-full text-xs font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                      />
                      <SelectField
                        value={model.protocol ?? 'openai-images'}
                        onValueChange={value => updateGenImageModel(index, { protocol: value })}
                        options={[
                          { value: 'openai-images', label: 'OpenAI Images' },
                          { value: 'qwen-openai-images', label: 'Qwen OpenAI Images' },
                          { value: 'gemini-generate-content', label: 'Gemini GenerateContent' },
                        ]}
                        className="w-full text-xs"
                        ariaLabel="生图协议"
                      />
                      <input
                        type="text"
                        value={model.url ?? ''}
                        onChange={event => updateGenImageModel(index, { url: event.target.value })}
                        placeholder="完整接口 URL，可使用 {model}"
                        className="input-field w-full text-xs font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                      />
                      <div className="grid grid-cols-2 gap-2">
                        <input
                          type="text"
                          value={model.model ?? ''}
                          onChange={event => updateGenImageModel(index, { model: event.target.value })}
                          placeholder="上游模型名"
                          className="input-field w-full text-xs font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                        />
                        <input
                          type="password"
                          value={model.apiKey ?? ''}
                          onChange={event => updateGenImageModel(index, { apiKey: event.target.value })}
                        placeholder="API Key"
                        autoComplete="off"
                        className="input-field w-full text-xs font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                        />
                      </div>
                      <details className="rounded-lg border border-gray-100 px-3 py-2 dark:border-white/10">
                        <summary className="cursor-pointer text-[11px] font-medium text-gray-600 dark:text-gray-300">
                          高级能力与编辑端点
                        </summary>
                        <div className="mt-3 grid gap-3">
                          <input
                            type="text"
                            value={model.editUrl ?? ''}
                            onChange={event => updateGenImageModel(index, { editUrl: event.target.value })}
                            placeholder="可选编辑接口 URL；标准协议可自动推导"
                            className="input-field w-full text-xs font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                          />
                          <div className="grid grid-cols-3 gap-2">
                            {(['generate', 'reference', 'edit'] as const).map(capability => (
                              <label key={capability} className="min-w-0">
                                <span className="mb-1 block text-[10px] text-gray-500 dark:text-gray-400">
                                  {capability === 'generate' ? '新图' : capability === 'reference' ? '参考图' : '编辑'}
                                </span>
                                <SelectField
                                  value={capabilityValue(model.capabilities?.[capability])}
                                  onValueChange={value => updateGenImageModel(index, {
                                    capabilities: {
                                      ...model.capabilities,
                                      [capability]: capabilityOverride(value),
                                    },
                                  })}
                                  options={CAPABILITY_OPTIONS}
                                  className="w-full text-xs"
                                  ariaLabel={`${capability} 能力`}
                                />
                              </label>
                            ))}
                          </div>
                          <div>
                            <span className="mb-1.5 block text-[10px] text-gray-500 dark:text-gray-400">偏好场景（可选）</span>
                            <div className="flex flex-wrap gap-1.5">
                              {VISUAL_SCENES.map(scene => {
                                const selected = model.capabilities?.preferredScenes?.includes(scene.value) ?? false;
                                return (
                                  <button
                                    key={scene.value}
                                    type="button"
                                    onClick={() => {
                                      const previous = model.capabilities?.preferredScenes ?? [];
                                      updateGenImageModel(index, {
                                        capabilities: {
                                          ...model.capabilities,
                                          preferredScenes: selected
                                            ? previous.filter(value => value !== scene.value)
                                            : [...previous, scene.value],
                                        },
                                      });
                                    }}
                                    className={`rounded-full px-2 py-1 text-[10px] ${selected
                                      ? 'bg-purple-100 text-purple-700 dark:bg-purple-500/20 dark:text-purple-300'
                                      : 'bg-gray-100 text-gray-500 dark:bg-white/10 dark:text-gray-400'}`}
                                  >
                                    {scene.label}
                                  </button>
                                );
                              })}
                            </div>
                          </div>
                        </div>
                      </details>
                    </div>
                  </div>
                ))}
                <button
                  type="button"
                  onClick={addGenImageModel}
                  className="mt-3 flex items-center gap-1.5 text-xs text-purple-600 hover:text-purple-700 dark:text-purple-300"
                >
                  <Plus size={13} /> 添加生图模型
                </button>
                <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-3 leading-relaxed">
                  所有模型走同一个 generate_image 工具；协议字段只负责适配上游请求格式。生图模型不会进入聊天 Provider 或顶部模型选择器。
                </p>
              </div>

              {/* tools.anySearch.apiKey */}
              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
                <div className="flex items-center gap-2 mb-3">
                  <Plug size={15} className="text-gray-500 dark:text-gray-400" />
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">AnySearch 联网搜索</h3>
                  <code className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-gray-100 dark:bg-white/10 text-gray-500 dark:text-gray-400">
                    tools.anySearch.apiKey
                  </code>
                </div>
                <input
                  type="password"
                  value={localAppConfig.anySearchApiKey ?? ''}
                  onChange={e => setLocalAppConfig(prev => ({ ...prev, anySearchApiKey: e.target.value }))}
                  placeholder={
                    (localConfig.tools?.anySearch?.apiKey ?? '') === LOCAL_CONFIG_TOKEN_MASK
                      ? '******'
                      : '可选 API Key（留空使用匿名额度）'
                  }
                  autoComplete="off"
                  className="input-field w-full text-sm font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                />
                <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-2 leading-relaxed">
                  AnySearch 是唯一联网搜索服务，并用于网页正文提取。留空使用匿名额度；API Key 仅保存在本机配置文件中。
                </p>
              </div>

              {/* server.showHiddenFiles */}
              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
                <div className="flex items-center gap-2 mb-3">
                  <Plug size={15} className="text-gray-500 dark:text-gray-400" />
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">显示隐藏文件</h3>
                  <code className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-gray-100 dark:bg-white/10 text-gray-500 dark:text-gray-400">
                    server.showHiddenFiles
                  </code>
                </div>
                <SelectField
                  value={(localAppConfig.showHiddenFiles ?? '') === 'true' ? 'true' : 'false'}
                  onValueChange={value => setLocalAppConfig(prev => ({ ...prev, showHiddenFiles: value }))}
                  options={[
                    { value: 'false', label: 'false' },
                    { value: 'true', label: 'true' },
                  ]}
                  className="w-full text-sm font-mono"
                  ariaLabel="是否显示隐藏文件"
                />
                <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-2 leading-relaxed">
                  控制所有本地文件浏览、目录选择和 Chat <code className="px-1 rounded bg-gray-100 dark:bg-white/10 font-mono">@</code> 引用入口是否显示 <code className="px-1 rounded bg-gray-100 dark:bg-white/10 font-mono">.</code> 开头的文件/文件夹。直接输入完整路径仍可访问。默认 <code className="px-1 rounded bg-gray-100 dark:bg-white/10 font-mono">false</code>。
                </p>
              </div>

              {/* Save bar */}
              <div className="flex items-center justify-between gap-3 pt-2">
                <p className="text-[11px] text-gray-400 dark:text-gray-500">
                  {isAppConfigDirty ? '有未保存的修改' : '所有更改已同步'}
                </p>
                <button
                  onClick={handleSaveAppConfig}
                  disabled={!isAppConfigDirty}
                  className={`px-4 py-2 text-sm rounded-lg transition-colors ${
                    isAppConfigDirty
                      ? 'bg-purple-500 text-white hover:bg-purple-600'
                      : 'bg-gray-100 dark:bg-white/5 text-gray-400 dark:text-gray-500 cursor-not-allowed'
                  }`}
                >
                  保存
                </button>
              </div>
            </div>
          )}

          {tab === 'memory' && (
            <MemorySettingsPanel />
          )}

          {tab === 'ssh' && (
            <div className="space-y-4">
              <div className="settings-note">
                <p className="text-xs leading-relaxed">
                  配置远程 SSH 服务器。AI 助手可通过 <code>ssh_execute</code> 等工具操作这些服务器。
                </p>
              </div>

              {/* Server List */}
              {sshServers.map((srv) => (
                <div key={srv.id} className="rounded-xl border border-gray-200 dark:border-white/10 p-4 bg-white dark:bg-white/[0.02]">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-2">
                      <MonitorSmartphone size={15} className="text-gray-500 dark:text-gray-400" />
                      <span className="text-sm font-medium text-gray-900 dark:text-white">{srv.alias}</span>
                      <span className="text-xs text-gray-400 dark:text-gray-500 font-mono">{srv.user}@{srv.host}:{srv.port}</span>
                      <span className={`text-[10px] px-1.5 py-0.5 rounded-full ${
                        srv.auth.type === 'key'
                          ? 'bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-400'
                          : 'bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400'
                      }`}>
                        {srv.auth.type === 'key' ? '密钥' : '密码'}
                      </span>
                    </div>
                    <div className="flex items-center gap-1">
                      <button
                        onClick={() => { setSshEditing({ ...srv }); setSshShowPassword(false); }}
                        className="p-1.5 rounded-lg hover:bg-gray-100 dark:hover:bg-white/10 transition-colors"
                      >
                        <Settings size={14} className="text-gray-400" />
                      </button>
                      <button
                        onClick={() => void sshSave(sshServers.filter(s => s.id !== srv.id))}
                        className="p-1.5 rounded-lg hover:bg-red-50 dark:hover:bg-red-900/20 transition-colors"
                      >
                        <Trash2 size={14} className="text-red-400" />
                      </button>
                    </div>
                  </div>
                  {srv.description && (
                    <p className="text-xs text-gray-400 dark:text-gray-500 mt-1">{srv.description}</p>
                  )}
                </div>
              ))}

              {/* Add button */}
              <button
                onClick={() => { setSshEditing(sshNewServer()); setSshShowPassword(false); }}
                className="w-full flex items-center justify-center gap-2 py-3 rounded-xl border border-dashed border-gray-300 dark:border-white/15 text-sm text-gray-500 dark:text-gray-400 hover:border-gray-400 dark:hover:border-white/25 hover:text-gray-800 dark:hover:text-gray-200 transition-colors"
              >
                <Plus size={16} />
                添加 SSH 服务器
              </button>

              {/* Edit modal */}
              {sshEditing && (
                <div className="rounded-xl border border-gray-300 dark:border-white/15 p-5 bg-gray-50/60 dark:bg-white/[0.025] space-y-3">
                  <h4 className="text-sm font-medium text-gray-900 dark:text-white">
                    {sshServers.some(s => s.id === sshEditing.id) ? '编辑服务器' : '新建服务器'}
                  </h4>

                  <div className="grid grid-cols-2 gap-3">
                    <div>
                      <label className="text-xs text-gray-500 dark:text-gray-400">别名 *</label>
                      <input
                        className="input-field w-full text-sm mt-1 dark:bg-white/5 dark:border-white/10 dark:text-white"
                        placeholder="prod-web-01"
                        value={sshEditing.alias}
                        onChange={e => setSshEditing({ ...sshEditing, alias: e.target.value })}
                      />
                    </div>
                    <div>
                      <label className="text-xs text-gray-500 dark:text-gray-400">主机 *</label>
                      <input
                        className="input-field w-full text-sm mt-1 dark:bg-white/5 dark:border-white/10 dark:text-white"
                        placeholder="192.168.1.100"
                        value={sshEditing.host}
                        onChange={e => setSshEditing({ ...sshEditing, host: e.target.value })}
                      />
                    </div>
                    <div>
                      <label className="text-xs text-gray-500 dark:text-gray-400">用户名 *</label>
                      <input
                        className="input-field w-full text-sm mt-1 dark:bg-white/5 dark:border-white/10 dark:text-white"
                        placeholder="root"
                        value={sshEditing.user}
                        onChange={e => setSshEditing({ ...sshEditing, user: e.target.value })}
                      />
                    </div>
                    <div>
                      <label className="text-xs text-gray-500 dark:text-gray-400">端口</label>
                      <input
                        type="number"
                        className="input-field w-full text-sm mt-1 dark:bg-white/5 dark:border-white/10 dark:text-white"
                        value={sshEditing.port}
                        onChange={e => setSshEditing({ ...sshEditing, port: parseInt(e.target.value) || 22 })}
                      />
                    </div>
                  </div>

                  {/* Auth type */}
                  <div>
                    <label className="text-xs text-gray-500 dark:text-gray-400">认证方式</label>
                    <div className="flex gap-2 mt-1">
                      <button
                        onClick={() => setSshEditing({ ...sshEditing, auth: { type: 'password', value: sshEditing.auth.type === 'password' ? sshEditing.auth.value : '' } })}
                        className={`px-3 py-1.5 text-xs rounded-lg border transition-colors ${
                          sshEditing.auth.type === 'password'
                            ? 'border-gray-500 bg-white dark:bg-white/10 text-gray-900 dark:text-white shadow-sm'
                            : 'border-gray-200 dark:border-white/10 text-gray-500 dark:text-gray-400'
                        }`}
                      >
                        密码
                      </button>
                      <button
                        onClick={() => setSshEditing({ ...sshEditing, auth: { type: 'key', path: sshEditing.auth.type === 'key' ? sshEditing.auth.path : '~/.ssh/id_rsa' } })}
                        className={`px-3 py-1.5 text-xs rounded-lg border transition-colors ${
                          sshEditing.auth.type === 'key'
                            ? 'border-gray-500 bg-white dark:bg-white/10 text-gray-900 dark:text-white shadow-sm'
                            : 'border-gray-200 dark:border-white/10 text-gray-500 dark:text-gray-400'
                        }`}
                      >
                        密钥
                      </button>
                    </div>
                  </div>

                  {/* Auth fields */}
                  {sshEditing.auth.type === 'password' ? (
                    <div>
                      <label className="text-xs text-gray-500 dark:text-gray-400">密码</label>
                      <div className="flex items-center gap-2 mt-1">
                        <input
                          type={sshShowPassword ? 'text' : 'password'}
                          className="input-field flex-1 text-sm font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                          value={sshEditing.auth.value}
                          onChange={e => setSshEditing({ ...sshEditing, auth: { type: 'password', value: e.target.value } })}
                        />
                        <button onClick={() => setSshShowPassword(!sshShowPassword)} className="p-2 rounded-lg hover:bg-gray-100 dark:hover:bg-white/10">
                          {sshShowPassword ? <EyeOff size={14} className="text-gray-400" /> : <Eye size={14} className="text-gray-400" />}
                        </button>
                      </div>
                    </div>
                  ) : (
                    <div>
                      <label className="text-xs text-gray-500 dark:text-gray-400">密钥路径</label>
                      <input
                        className="input-field w-full text-sm font-mono mt-1 dark:bg-white/5 dark:border-white/10 dark:text-white"
                        placeholder="~/.ssh/id_rsa"
                        value={sshEditing.auth.path}
                        onChange={e => setSshEditing({ ...sshEditing, auth: { type: 'key', path: e.target.value } })}
                      />
                    </div>
                  )}

                  {/* Description */}
                  <div>
                    <label className="text-xs text-gray-500 dark:text-gray-400">备注</label>
                    <input
                      className="input-field w-full text-sm mt-1 dark:bg-white/5 dark:border-white/10 dark:text-white"
                      placeholder="生产环境 Web 服务器"
                      value={sshEditing.description}
                      onChange={e => setSshEditing({ ...sshEditing, description: e.target.value })}
                    />
                  </div>

                  {/* Actions */}
                  <div className="flex justify-end gap-2 pt-2">
                    <button
                      onClick={() => setSshEditing(null)}
                      className="px-4 py-2 text-xs rounded-lg border border-gray-200 dark:border-white/10 text-gray-600 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-white/5 transition-colors"
                    >
                      取消
                    </button>
                    <button
                      onClick={async () => {
                        if (!sshEditing.alias || !sshEditing.host || !sshEditing.user) {
                          showToast({ message: '别名、主机、用户名必填', type: 'error' });
                          return;
                        }
                        const updated = { ...sshEditing, updatedAt: new Date().toISOString() };
                        const idx = sshServers.findIndex(s => s.id === updated.id);
                        let saved = false;
                        if (idx >= 0) {
                          const next = [...sshServers];
                          next[idx] = updated;
                          saved = await sshSave(next);
                        } else {
                          saved = await sshSave([...sshServers, updated]);
                        }
                        if (saved) {
                          setSshEditing(null);
                          showToast({ message: `SSH 服务器 ${updated.alias} 已保存`, type: 'success' });
                        }
                      }}
                      className="px-4 py-2 text-xs rounded-lg bg-purple-600 text-white hover:bg-purple-700 transition-colors"
                    >
                      保存
                    </button>
                  </div>
                </div>
              )}
            </div>
          )}

          {tab === 'system' && (
            <div className="space-y-4">
              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
                <div className="flex items-center gap-2 mb-4">
                  <Server size={16} className="text-gray-500 dark:text-gray-400" />
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">pwcli daemon</h3>
                </div>
                <div className="space-y-3">
                  <div className="flex items-center justify-between py-1">
                    <span className="text-sm text-gray-600 dark:text-gray-300">状态</span>
                    <button
                      onClick={checkDaemon}
                      className={`text-sm flex items-center gap-1.5 font-medium ${
                        daemonStatus?.healthy ? 'text-green-600 dark:text-green-400' :
                        daemonCheckFailed ? 'text-red-500 dark:text-red-400' : 'text-gray-400 dark:text-gray-500'
                      }`}>
                      {daemonStatus?.healthy ? <CheckCircle size={15} /> :
                       daemonCheckFailed ? <AlertCircle size={15} /> : null}
                      {daemonStatus?.healthy ? '正常' : daemonCheckFailed ? '无法连接' : '检测中...'}
                    </button>
                  </div>
                  <div className="flex items-center justify-between py-1">
                    <span className="text-sm text-gray-600 dark:text-gray-300">运行形态</span>
                    <span className="text-sm text-gray-800 dark:text-gray-200 font-medium">
                      单二进制（Web / API / Agent）
                    </span>
                  </div>
                  {daemonStatus && <>
                    <div className="flex items-center justify-between gap-4 py-1">
                      <span className="text-sm text-gray-600 dark:text-gray-300">地址</span>
                      <code className="text-xs text-gray-700 dark:text-gray-300">{daemonStatus.httpAddress}</code>
                    </div>
                    <div className="flex items-center justify-between gap-4 py-1">
                      <span className="text-sm text-gray-600 dark:text-gray-300">版本</span>
                      <code className="max-w-[70%] truncate text-xs text-gray-700 dark:text-gray-300" title={daemonStatus.version}>{daemonStatus.version}</code>
                    </div>
                  </>}
                </div>
              </div>

              <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
                <div className="flex items-center gap-2 mb-4">
                  <Sparkles size={16} className="text-gray-500 dark:text-gray-400" />
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">功能开关</h3>
                </div>
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-medium text-gray-800 dark:text-gray-200">Mermaid 流程图</div>
                    <p className="text-xs text-gray-400 dark:text-gray-500 mt-1 leading-relaxed">
                      渲染 Mermaid 代码块为流程图；关闭可节省 ~80MB 内存，刷新页面后生效
                    </p>
                  </div>
                  <button
                    role="switch"
                    aria-checked={mermaidEnabled}
                    onClick={() => handleToggleMermaid(!mermaidEnabled)}
                    className={`relative shrink-0 inline-flex items-center w-11 h-6 rounded-full transition-colors duration-200 ease-in-out ${
                      mermaidEnabled ? 'bg-purple-500' : 'bg-gray-300 dark:bg-white/15'
                    }`}
                  >
                    <span
                      aria-hidden="true"
                      className={`inline-block h-5 w-5 rounded-full bg-white shadow transform transition-transform duration-200 ease-in-out ${
                        mermaidEnabled ? 'translate-x-[22px]' : 'translate-x-0.5'
                      }`}
                    />
                  </button>
                </div>
              </div>
            </div>
          )}

          {tab === 'about' && (
            <div className="text-center py-12 space-y-3">
              <h3 className="text-xl font-semibold text-gray-900 dark:text-white">个人工作台</h3>
              <p className="text-sm text-gray-500 dark:text-gray-400">版本 0.1.0</p>
              <p className="text-xs text-gray-400 dark:text-gray-500 max-w-sm mx-auto leading-relaxed mt-2">
                基于 React + TypeScript + Vite + Rust 构建的个人效率工具。
                数据本地存储，AI 配置仅保存在当前设备。
              </p>
              <p className="text-xs text-gray-400 dark:text-gray-500">AGPL-3.0-only · 公开发行版随对应 commit 提供完整源码</p>
              <div className="flex items-center justify-center gap-3 text-xs">
                <a className="text-purple-600 hover:underline dark:text-purple-400" href="https://www.gnu.org/licenses/agpl-3.0.html" target="_blank" rel="noreferrer">开源许可证</a>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Edit Provider Modal */}
      {editingIndex !== null && (
        <div className="settings-overlay fixed inset-0 z-[110] flex items-center justify-center">
          <div className="absolute inset-0 bg-black/50 backdrop-blur-sm dark:bg-black/70" onClick={() => setEditingIndex(null)} />
          <div
            ref={providerDialogRef}
            className="provider-dialog relative w-[620px] max-h-[90vh] flex flex-col rounded-2xl border shadow-2xl overflow-hidden bg-white dark:bg-[#1a1a1a]"
            role="dialog"
            aria-modal="true"
            aria-label={editingIndex === -1 ? '添加 AI Provider' : '编辑 AI Provider'}
            tabIndex={-1}
            style={{ borderColor: 'rgba(0,0,0,0.08)' }}
          >
            <div className="flex items-center justify-between px-6 py-4 border-b border-gray-200 dark:border-white/10">
              <h3 className="text-base font-semibold text-gray-900 dark:text-white">
                {editingIndex === -1 ? '添加 Provider' : '编辑 Provider'}
              </h3>
              <button onClick={() => setEditingIndex(null)} className="p-1.5 rounded-lg hover:bg-gray-100 dark:hover:bg-white/10 transition-colors" aria-label="关闭 Provider 编辑">
                <X size={18} className="text-gray-400 dark:text-gray-500" />
              </button>
            </div>

            <div className="flex-1 overflow-y-auto p-6 space-y-5">
              {/* Basic Info */}
              <div className="grid grid-cols-2 gap-4">
                <div>
                  <label className="block text-xs font-medium text-gray-600 dark:text-gray-300 mb-1.5">名称 <span className="text-red-500">*</span></label>
                  <input
                    type="text"
                    value={editForm.name}
                    onChange={e => setEditForm({ ...editForm, name: e.target.value })}
                    placeholder="例如：OpenAI"
                    className="input-field w-full text-sm dark:bg-white/5 dark:border-white/10 dark:text-white"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium text-gray-600 dark:text-gray-300 mb-1.5">协议 <span className="text-red-500">*</span></label>
                  <SelectField
                    value={editForm.protocol}
                    onValueChange={value => setEditForm({ ...editForm, protocol: value as 'openai' | 'anthropic' })}
                    options={[
                      { value: 'openai', label: 'OpenAI' },
                      { value: 'anthropic', label: 'Anthropic' },
                    ]}
                    className="w-full text-sm"
                    ariaLabel="Provider 协议"
                  />
                </div>
              </div>

              <div>
                <label className="block text-xs font-medium text-gray-600 dark:text-gray-300 mb-1.5">Base URL <span className="text-red-500">*</span></label>
                <input
                  type="text"
                  value={editForm.baseUrl}
                  onChange={e => setEditForm({ ...editForm, baseUrl: e.target.value })}
                  placeholder="https://api.openai.com/v1"
                  className="input-field w-full text-sm dark:bg-white/5 dark:border-white/10 dark:text-white"
                />
                <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-1">末尾不要带斜杠，例如 https://api.openai.com/v1</p>
              </div>

              <div>
                <label className="block text-xs font-medium text-gray-600 dark:text-gray-300 mb-1.5">API Key <span className="text-red-500">*</span></label>
                <input
                  type="password"
                  value={editForm.apiKey}
                  onChange={e => setEditForm({ ...editForm, apiKey: e.target.value })}
                  placeholder="sk-..."
                  className="input-field w-full text-sm dark:bg-white/5 dark:border-white/10 dark:text-white"
                />
                <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-1">仅保存在本地，不会上传或分发</p>
              </div>

              {/* Model List Editor */}
              <div>
                <div className="flex items-center justify-between mb-2">
                  <label className="block text-xs font-medium text-gray-600 dark:text-gray-300">模型列表</label>
                  <div className="flex items-center gap-1.5">
                    <button
                      onClick={addModel}
                      className="flex items-center gap-1 px-2 py-1 text-[11px] font-medium rounded-md bg-purple-500/10 text-purple-600 dark:text-purple-400 hover:bg-purple-500/20 transition-colors"
                    >
                      <Plus size={12} />
                      添加模型
                    </button>
                  </div>
                </div>

                {editForm.models.length === 0 && (
                  <div className="text-center py-6 text-gray-400 dark:text-gray-500 bg-gray-50 dark:bg-white/[0.03] rounded-xl border border-dashed border-gray-200 dark:border-white/10">
                    <p className="text-sm">还没有模型</p>
                    <p className="text-xs mt-0.5">点击上方按钮添加</p>
                  </div>
                )}

                <div className="space-y-3" ref={modelListRef}>
                  {editForm.models.map((m, idx) => (
                    <div key={modelRowKeysRef.current[idx]} className="space-y-1.5">
                      <div className="flex items-center gap-2">
                        <GripVertical size={14} className="model-drag-handle text-gray-300 dark:text-gray-600 shrink-0 cursor-grab active:cursor-grabbing" />
                        <input
                          type="text"
                          value={m.id}
                          onChange={e => updateModel(idx, 'id', e.target.value)}
                          placeholder="模型 ID，如 gpt-4"
                          className="input-field flex-1 text-sm dark:bg-white/5 dark:border-white/10 dark:text-white"
                        />
                        <input
                          type="text"
                          value={m.name}
                          onChange={e => updateModel(idx, 'name', e.target.value)}
                          placeholder="显示名称，如 GPT-4"
                          className="input-field flex-1 text-sm dark:bg-white/5 dark:border-white/10 dark:text-white"
                        />
                        <button
                          onClick={() => removeModel(idx)}
                          className="p-1.5 rounded-md hover:bg-red-50 dark:hover:bg-red-500/10 text-red-500 transition-colors shrink-0"
                          title="删除"
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                      <div className="flex items-center gap-4 ml-6 text-xs text-gray-500 dark:text-gray-400 flex-wrap">
                        <label className="inline-flex items-center gap-1.5 cursor-pointer select-none hover:text-gray-700 dark:hover:text-gray-200 transition-colors">
                          <input
                            type="checkbox"
                            checked={m.enabled !== false}
                            onChange={e => updateModelEnabled(idx, e.target.checked)}
                            className="w-3.5 h-3.5 accent-purple-500 cursor-pointer"
                          />
                          <span>显示在选择器</span>
                        </label>
                        <label className="inline-flex items-center gap-1.5 cursor-pointer select-none hover:text-gray-700 dark:hover:text-gray-200 transition-colors">
                          <input
                            type="checkbox"
                            checked={!!m.capabilities?.vision}
                            onChange={e => updateModelCapability(idx, 'vision', e.target.checked)}
                            className="w-3.5 h-3.5 accent-purple-500 cursor-pointer"
                          />
                          <span>视觉（图片输入）</span>
                        </label>
                        <label className="inline-flex items-center gap-1.5 cursor-pointer select-none hover:text-gray-700 dark:hover:text-gray-200 transition-colors">
                          <input
                            type="checkbox"
                            checked={!!m.capabilities?.thinking}
                            onChange={e => updateModelCapability(idx, 'thinking', e.target.checked)}
                            className="w-3.5 h-3.5 accent-purple-500 cursor-pointer"
                          />
                          <span>思考（深度推理）</span>
                        </label>
                        {supportsKimiDeferredTools(editForm) && (
                          <label className="inline-flex items-center gap-1.5 cursor-pointer select-none hover:text-gray-700 dark:hover:text-gray-200 transition-colors">
                            <input
                              type="checkbox"
                              checked={m.deferredToolsMode === 'kimi'}
                              onChange={e => updateModelDeferredTools(idx, e.target.checked)}
                              className="w-3.5 h-3.5 accent-purple-500 cursor-pointer"
                            />
                            <span>Kimi 延迟工具</span>
                          </label>
                        )}
                        <label className="inline-flex items-center gap-1.5 select-none">
                          <span>最大输出</span>
                          <input
                            type="number"
                            min={1}
                            value={m.maxOutput ?? ''}
                            placeholder={String(getDefaultMaxOutput(m.id))}
                            onChange={e => updateModelMaxOutput(idx, e.target.value)}
                            className="input-field w-24 text-xs px-2 py-1 dark:bg-white/5 dark:border-white/10 dark:text-white"
                          />
                          <span className="text-gray-400 dark:text-gray-500">tokens</span>
                          <button
                            type="button"
                            disabled={!getModelMeta(m.id)}
                            title={
                              getModelMeta(m.id)
                                ? `按已知模型规格自动填入 ${getModelMeta(m.id)!.maxOutput} tokens`
                                : '该模型不在已知静态表中，无法自动填入'
                            }
                            onClick={() => {
                              const meta = getModelMeta(m.id);
                              if (meta) updateModelMaxOutput(idx, String(meta.maxOutput));
                            }}
                            className="px-1.5 py-0.5 text-[10px] rounded border border-purple-400/40 text-purple-500 dark:text-purple-300 hover:bg-purple-500/10 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                          >
                            自动
                          </button>
                        </label>
                        <label className="inline-flex items-center gap-1.5 select-none">
                          <span>上下文窗口</span>
                          <input
                            type="number"
                            min={1}
                            value={m.contextWindow ?? ''}
                            placeholder={String(getModelMeta(m.id)?.contextWindow ?? 200000)}
                            onChange={e => updateModelContextWindow(idx, e.target.value)}
                            className="input-field w-28 text-xs px-2 py-1 dark:bg-white/5 dark:border-white/10 dark:text-white"
                          />
                          <span className="text-gray-400 dark:text-gray-500">tokens</span>
                          <button
                            type="button"
                            disabled={!getModelMeta(m.id)}
                            title={getModelMeta(m.id)
                              ? `按已知模型规格自动填入 ${getModelMeta(m.id)!.contextWindow} tokens`
                              : '该模型不在已知静态表中，请手动填写'}
                            onClick={() => {
                              const meta = getModelMeta(m.id);
                              if (meta) updateModelContextWindow(idx, String(meta.contextWindow));
                            }}
                            className="px-1.5 py-0.5 text-[10px] rounded border border-purple-400/40 text-purple-500 dark:text-purple-300 hover:bg-purple-500/10 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                          >
                            自动
                          </button>
                        </label>
                      </div>
                      <div className="grid grid-cols-2 gap-2 ml-6">
                        <JsonParamsEditor
                          label="每次请求参数（JSON）"
                          value={m.requestParams}
                          placeholder={'{"top_p": 0.95}'}
                          onChange={value => updateModelParams(idx, 'requestParams', value)}
                        />
                        <JsonParamsEditor
                          label="开启思考时参数（JSON）"
                          value={m.thinkingParams}
                          placeholder={'{"reasoning_effort": "max"}'}
                          onChange={value => updateModelParams(idx, 'thinkingParams', value)}
                        />
                      </div>
                    </div>
                  ))}
                </div>

                <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-2">
                  模型 ID 用于 API 调用，显示名称用于下拉选择。所有修改会同步写回 <code className="px-1 rounded bg-purple-500/10 text-purple-400 font-mono">~/.pwcli/config.json</code>。
                </p>
              </div>
            </div>

            <div className="flex justify-end gap-2 px-6 py-4 border-t border-gray-200 dark:border-white/10 bg-gray-50 dark:bg-white/[0.02]">
              <button
                onClick={() => setEditingIndex(null)}
                className="px-4 py-2 text-sm rounded-lg border border-gray-200 dark:border-white/10 hover:bg-gray-100 dark:hover:bg-white/10 text-gray-700 dark:text-gray-300 transition-colors"
              >
                取消
              </button>
              <button
                onClick={handleSaveEdit}
                className="px-4 py-2 text-sm rounded-lg bg-purple-500 text-white hover:bg-purple-600 transition-colors"
              >
                保存
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
