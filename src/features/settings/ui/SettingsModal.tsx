import { useState, useEffect, useCallback } from 'react';
import { X, Plus, Trash2, Settings, Server, CheckCircle, AlertCircle, Sparkles, Plug, RotateCcw, MonitorSmartphone, Eye, EyeOff, FolderOpen } from 'lucide-react';
import { MemorySettingsPanel } from '@/features/memory';
import type { FeatureModelKey, AppConfig, LocalConfig, LocalConfigGenImageModel, LocalConfigSshServer, ResponseLanguage, VisualScene } from '@/core/config';
import {
  useAiModels, FEATURE_MODELS, getFeatureModel, setFeatureModel,
  useAppConfig, setAppConfig,
  useLocalConfig, saveLocalConfig, LOCAL_CONFIG_TOKEN_MASK, normalizeFsBase,
  useMoaConfig, saveMoaConfig, getMoaConfig, getProviderModelOptions, validateMoaConfig,
} from '@/core/config';
import HarnessMoaSettingsPanel from './HarnessMoaSettingsPanel';
import ProviderSettingsPanel from './ProviderSettingsPanel';
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

export default function SettingsModal({ open, onClose }: Props) {
  const [tab, setTab] = useState<Tab>('ai');
  const aiModels = useAiModels();
  const providerModelOptions = getProviderModelOptions();
  const moaConfig = useMoaConfig();
  const [localMoaConfig, setLocalMoaConfig] = useState(moaConfig);
  const moaConfigErrors = validateMoaConfig(localMoaConfig, providerModelOptions);
  const isMoaConfigDirty = JSON.stringify(localMoaConfig) !== JSON.stringify(moaConfig);
  const settingsDialogRef = useModalDialog({ open, onClose });
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




  // Refresh model selections when another browser/tab pushes configuration changes.
  useStorageSync(() => {
    if (!open) return;
    setFeatureModelMap(
      Object.fromEntries(FEATURE_MODELS.map(f => [f.key, getFeatureModel(f.key)])) as Record<FeatureModelKey, string>
    );
  });

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
            <ProviderSettingsPanel />
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
                数据本地存储，AI 凭据由本机 daemon 安全管理。
              </p>
              <p className="text-xs text-gray-400 dark:text-gray-500">AGPL-3.0-only · 公开发行版随对应 commit 提供完整源码</p>
              <div className="flex items-center justify-center gap-3 text-xs">
                <a className="text-purple-600 hover:underline dark:text-purple-400" href="https://www.gnu.org/licenses/agpl-3.0.html" target="_blank" rel="noreferrer">开源许可证</a>
              </div>
            </div>
          )}
        </div>
      </div>

    </div>
  );
}
