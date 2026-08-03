import { useEffect, useMemo, useState } from 'react';
import {
  AlertCircle,
  ArrowDown,
  ArrowUp,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Copy,
  ExternalLink,
  KeyRound,
  Loader2,
  LogOut,
  Plus,
  RefreshCw,
  Server,
  Trash2,
  X,
} from 'lucide-react';
import {
  PROVIDER_PROTOCOL_OPTIONS,
  cancelProviderAuth,
  completeProviderAuth,
  createBuiltinProvider,
  deleteProvider,
  getProviderSnapshot,
  getProviderAuthStatus,
  logoutProvider,
  refreshProviders,
  reorderProviderIds,
  saveCustomProvider,
  startProviderAuth,
  testProvider,
  useProviderStore,
  type CustomProviderInput,
  type ProviderAuthFlow,
  type ProviderCatalogEntry,
  type ProviderView,
} from '@/core/config';
import { showToast } from '@/shell';

const EMPTY_CUSTOM: CustomProviderInput = {
  name: '', baseUrl: '', protocol: 'openai_chat', apiKey: '', defaultModel: '', models: [],
};

function statusView(provider: ProviderView): { text: string; tone: string } {
  switch (provider.auth.status) {
    case 'connected': return { text: '已连接', tone: 'text-emerald-600 bg-emerald-500/10' };
    case 'pending': return { text: '等待授权', tone: 'text-amber-600 bg-amber-500/10' };
    case 'expired': return { text: '需要重新登录', tone: 'text-amber-600 bg-amber-500/10' };
    case 'error': return { text: '连接异常', tone: 'text-red-600 bg-red-500/10' };
    default: return { text: '未连接', tone: 'text-gray-500 bg-gray-500/10' };
  }
}

function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: React.ReactNode }) {
  return <div className="fixed inset-0 z-[130] flex items-center justify-center p-4">
    <button className="absolute inset-0 bg-black/55 backdrop-blur-sm" onClick={onClose} aria-label="关闭" />
    <section className="relative flex max-h-[88vh] w-full max-w-xl flex-col overflow-hidden rounded-2xl border border-gray-200 bg-white shadow-2xl dark:border-white/10 dark:bg-[#1a1a1a]" role="dialog" aria-modal="true" aria-label={title}>
      <header className="flex items-center justify-between border-b border-gray-200 px-5 py-4 dark:border-white/10">
        <h3 className="font-semibold text-gray-900 dark:text-white">{title}</h3>
        <button onClick={onClose} className="rounded-lg p-1.5 text-gray-400 hover:bg-gray-100 dark:hover:bg-white/10" aria-label="关闭"><X size={18} /></button>
      </header>
      {children}
    </section>
  </div>;
}

export default function ProviderSettingsPanel() {
  const { providers, catalog, loading, error } = useProviderStore();
  const [adding, setAdding] = useState(false);
  const [custom, setCustom] = useState<CustomProviderInput | null>(null);
  const [customId, setCustomId] = useState<string>();
  const [advanced, setAdvanced] = useState(false);
  const [modelsDraft, setModelsDraft] = useState('');
  const [qwenKey, setQwenKey] = useState('');
  const [qwenEntry, setQwenEntry] = useState<ProviderCatalogEntry>();
  const [authProvider, setAuthProvider] = useState<ProviderView>();
  const [authFlow, setAuthFlow] = useState<ProviderAuthFlow>();
  const [authMethod, setAuthMethod] = useState<'device' | 'browser'>('device');
  const [manualCode, setManualCode] = useState('');
  const [busy, setBusy] = useState(false);

  const connectedKinds = useMemo(() => new Set(providers.filter(p => p.builtin).map(p => p.kind)), [providers]);

  useEffect(() => {
    if (!authProvider || !authFlow?.flowId || authFlow.status !== 'pending') return;
    const interval = Math.max(1500, authFlow.pollIntervalMs ?? 3000);
    const timer = window.setInterval(() => {
      void getProviderAuthStatus(authProvider.id, authFlow.flowId).then(next => {
        setAuthFlow(next);
        if (next.status === 'connected') {
          window.clearInterval(timer);
          void refreshProviders();
          showToast({ message: `${authProvider.name} 已连接`, type: 'success' });
        }
      }).catch(error => setAuthFlow(flow => flow ? { ...flow, status: 'error', error: error instanceof Error ? error.message : '授权状态检查失败' } : flow));
    }, interval);
    return () => window.clearInterval(timer);
  }, [authFlow?.flowId, authFlow?.pollIntervalMs, authFlow?.status, authProvider]);

  const perform = async (action: () => Promise<void>, success?: string) => {
    setBusy(true);
    try {
      await action();
      if (success) showToast({ message: success, type: 'success' });
    } catch (cause) {
      showToast({ message: cause instanceof Error ? cause.message : '操作失败', type: 'error' });
    } finally { setBusy(false); }
  };

  const beginBuiltin = async (entry: ProviderCatalogEntry) => {
    if (entry.kind === 'qwen-token-plan-cn') { setQwenEntry(entry); setAdding(false); return; }
    await perform(async () => {
      if (!connectedKinds.has(entry.kind)) await createBuiltinProvider(entry.kind);
      let resolved = providers.find(p => p.kind === entry.kind);
      if (!resolved) {
        await refreshProviders();
        resolved = getProviderSnapshot().providers.find(p => p.kind === entry.kind);
      }
      if (!resolved) throw new Error('服务已创建，但尚未出现在 Provider 列表中');
      setAuthProvider(resolved);
      setAuthMethod(entry.kind === 'openai-codex' ? 'browser' : 'device');
      setAuthFlow(undefined);
      setAdding(false);
    });
  };

  const beginAuth = async () => {
    if (!authProvider) return;
    await perform(async () => {
      const flow = await startProviderAuth(authProvider.id, authMethod);
      setAuthFlow(flow);
      if (authMethod === 'browser' && flow.verificationUri) window.open(flow.verificationUri, '_blank', 'noopener,noreferrer');
    });
  };

  const closeAuth = async () => {
    if (authProvider && authFlow?.flowId && authFlow.status === 'pending') {
      await cancelProviderAuth(authProvider.id, authFlow.flowId).catch(() => undefined);
    }
    setAuthProvider(undefined); setAuthFlow(undefined); setManualCode('');
  };

  const openCustom = (provider?: ProviderView) => {
    setAdding(false); setAdvanced(false); setCustomId(provider?.id);
    const models = provider?.models ?? [];
    setModelsDraft(models.map(model => `${model.id}|${model.name || model.id}`).join('\n'));
    setCustom(provider ? {
      name: provider.name, baseUrl: provider.baseUrl ?? '', protocol: (provider.protocol === 'openai' ? 'openai_chat' : provider.protocol === 'anthropic' ? 'anthropic_messages' : provider.protocol) ?? 'openai_chat',
      defaultModel: provider.defaultModel ?? models[0]?.id ?? '', models, useProxy: provider.useProxy, apiKey: '',
    } : { ...EMPTY_CUSTOM });
  };

  const saveCustom = async () => {
    if (!custom || !custom.name.trim() || !custom.baseUrl.trim() || !custom.defaultModel.trim()) {
      showToast({ message: '请填写名称、API 地址和默认模型', type: 'error' }); return;
    }
    const models = modelsDraft.split('\n').map(line => line.trim()).filter(Boolean).map(line => {
      const [id, name] = line.split('|').map(part => part.trim());
      return { id, name: name || id, enabled: true };
    });
    if (!models.some(model => model.id === custom.defaultModel)) models.unshift({ id: custom.defaultModel, name: custom.defaultModel, enabled: true });
    await perform(async () => { await saveCustomProvider({ ...custom, models }, customId); setCustom(null); }, customId ? '自定义服务已更新' : '自定义服务已添加');
  };

  const move = (index: number, direction: -1 | 1) => {
    const next = [...providers];
    const target = index + direction;
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target], next[index]];
    void perform(() => reorderProviderIds(next.map(item => item.id)), '优先级已更新');
  };

  return <div className="space-y-4">
    <div className="flex items-start justify-between gap-4">
      <div>
        <p className="text-sm text-gray-700 dark:text-gray-200">连接 AI 服务</p>
        <p className="mt-1 text-xs text-gray-400">登录或填写 API Key 后选择模型；认证凭据只保存在 daemon 中。</p>
      </div>
      <button onClick={() => setAdding(true)} className="flex items-center gap-1.5 rounded-lg bg-purple-500 px-3 py-2 text-xs font-medium text-white hover:bg-purple-600"><Plus size={14} />添加 AI 服务</button>
    </div>

    {error && <div className="flex items-center justify-between rounded-xl border border-amber-200 bg-amber-50 p-3 text-xs text-amber-700 dark:border-amber-500/20 dark:bg-amber-500/10 dark:text-amber-300"><span className="flex items-center gap-2"><AlertCircle size={14} />{error}</span><button onClick={() => void refreshProviders()} className="underline">重试</button></div>}
    {loading && providers.length === 0 && <div className="flex justify-center py-10 text-gray-400"><Loader2 className="animate-spin" /></div>}
    {!loading && !error && providers.length === 0 && <button onClick={() => setAdding(true)} className="w-full rounded-xl border border-dashed border-gray-300 py-10 text-sm text-gray-400 hover:border-purple-400 hover:text-purple-500 dark:border-white/15">尚未连接 AI 服务，点击添加</button>}

    <div className="space-y-3">
      {providers.map((provider, index) => {
        const status = statusView(provider);
        return <article key={provider.id} className="rounded-xl border border-gray-200 bg-white p-4 dark:border-white/10 dark:bg-white/[0.02]">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-[10px] font-medium text-purple-500">{index + 1}</span>
                <strong className="truncate text-sm text-gray-900 dark:text-white">{provider.name}</strong>
                <span className={`rounded-md px-2 py-0.5 text-[10px] ${status.tone}`}>{status.text}</span>
                {!provider.builtin && <span className="rounded-md bg-gray-100 px-2 py-0.5 text-[10px] text-gray-500 dark:bg-white/10">自定义</span>}
              </div>
              <p className="mt-1.5 text-xs text-gray-400">{provider.defaultModel || provider.models[0]?.name || '尚未选择模型'} · {provider.models.length} 个模型{provider.auth.accountLabel ? ` · ${provider.auth.accountLabel}` : ''}</p>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <button onClick={() => move(index, -1)} disabled={index === 0 || busy} className="rounded p-1.5 text-gray-400 hover:bg-gray-100 disabled:opacity-25 dark:hover:bg-white/10" aria-label={`上移 ${provider.name}`}><ArrowUp size={14} /></button>
              <button onClick={() => move(index, 1)} disabled={index === providers.length - 1 || busy} className="rounded p-1.5 text-gray-400 hover:bg-gray-100 disabled:opacity-25 dark:hover:bg-white/10" aria-label={`下移 ${provider.name}`}><ArrowDown size={14} /></button>
            </div>
          </div>
          <div className="mt-3 flex flex-wrap gap-2 border-t border-gray-100 pt-3 dark:border-white/5">
            {provider.auth.method === 'oauth' && provider.auth.status !== 'connected' && <button onClick={() => { setAuthProvider(provider); setAuthMethod(provider.kind === 'openai-codex' ? 'browser' : 'device'); setAuthFlow(undefined); }} className="rounded-md bg-purple-500/10 px-2.5 py-1 text-xs text-purple-600 dark:text-purple-300"><KeyRound size={12} className="mr-1 inline" />登录</button>}
            <button onClick={() => void perform(() => testProvider(provider.id), '连接测试成功')} className="rounded-md px-2.5 py-1 text-xs text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-white/10"><RefreshCw size={12} className="mr-1 inline" />测试连接</button>
            {!provider.builtin && <button onClick={() => openCustom(provider)} className="rounded-md px-2.5 py-1 text-xs text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-white/10">编辑</button>}
            {provider.auth.method === 'oauth' && provider.auth.status === 'connected' && <button onClick={() => void perform(() => logoutProvider(provider.id), '已退出登录')} className="rounded-md px-2.5 py-1 text-xs text-gray-500 hover:bg-gray-100 dark:hover:bg-white/10"><LogOut size={12} className="mr-1 inline" />退出</button>}
            <button onClick={() => void perform(() => deleteProvider(provider.id), 'AI 服务已删除')} className="ml-auto rounded-md p-1 text-red-500 hover:bg-red-500/10" aria-label={`删除 ${provider.name}`}><Trash2 size={14} /></button>
          </div>
        </article>;
      })}
    </div>

    {adding && <Modal title="添加 AI 服务" onClose={() => setAdding(false)}><div className="grid gap-3 overflow-y-auto p-5 sm:grid-cols-2">
      {catalog.map(entry => <button key={entry.kind} disabled={connectedKinds.has(entry.kind)} onClick={() => void beginBuiltin(entry)} className="rounded-xl border border-gray-200 p-4 text-left hover:border-purple-400 hover:bg-purple-500/[0.03] disabled:cursor-not-allowed disabled:opacity-45 dark:border-white/10">
        <span className="flex items-center gap-2 text-sm font-medium text-gray-900 dark:text-white"><Server size={16} className="text-purple-500" />{entry.name}</span>
        <span className="mt-2 block text-xs leading-5 text-gray-400">{connectedKinds.has(entry.kind) ? '已添加' : entry.description}</span>
      </button>)}
      <button onClick={() => openCustom()} className="rounded-xl border border-gray-200 p-4 text-left hover:border-purple-400 hover:bg-purple-500/[0.03] dark:border-white/10"><span className="flex items-center gap-2 text-sm font-medium text-gray-900 dark:text-white"><Plus size={16} className="text-purple-500" />自定义 Provider</span><span className="mt-2 block text-xs leading-5 text-gray-400">OpenAI、Anthropic、Gemini 或兼容网关</span></button>
    </div></Modal>}

    {qwenEntry && <Modal title={`连接 ${qwenEntry.name}`} onClose={() => { setQwenEntry(undefined); setQwenKey(''); }}><div className="space-y-4 p-5"><label className="block text-xs text-gray-600 dark:text-gray-300">API Key<input autoFocus type="password" value={qwenKey} onChange={event => setQwenKey(event.target.value)} className="input-field mt-1.5 w-full dark:bg-white/5 dark:border-white/10 dark:text-white" placeholder="sk-..." /></label><p className="text-xs text-gray-400">密钥将直接提交给本机 daemon，不会写入浏览器存储。</p><div className="flex justify-end gap-2"><button onClick={() => setQwenEntry(undefined)} className="rounded-lg border px-4 py-2 text-sm dark:border-white/10">取消</button><button disabled={!qwenKey.trim() || busy} onClick={() => void perform(async () => { await createBuiltinProvider(qwenEntry.kind, qwenKey); setQwenEntry(undefined); setQwenKey(''); }, 'Qwen Token Plan CN 已连接')} className="rounded-lg bg-purple-500 px-4 py-2 text-sm text-white disabled:opacity-50">保存并连接</button></div></div></Modal>}

    {custom && <Modal title={customId ? '编辑自定义 Provider' : '添加自定义 Provider'} onClose={() => setCustom(null)}><div className="space-y-4 overflow-y-auto p-5">
      <div className="grid gap-3 sm:grid-cols-2"><label className="text-xs text-gray-600 dark:text-gray-300">名称<input value={custom.name} onChange={e => setCustom({ ...custom, name: e.target.value })} className="input-field mt-1.5 w-full dark:bg-white/5 dark:border-white/10 dark:text-white" /></label><label className="text-xs text-gray-600 dark:text-gray-300">API 类型<select value={custom.protocol} onChange={e => setCustom({ ...custom, protocol: e.target.value as CustomProviderInput['protocol'] })} className="input-field mt-1.5 w-full dark:bg-white/5 dark:border-white/10 dark:text-white">{PROVIDER_PROTOCOL_OPTIONS.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label></div>
      <label className="block text-xs text-gray-600 dark:text-gray-300">API 地址<input value={custom.baseUrl} onChange={e => setCustom({ ...custom, baseUrl: e.target.value })} className="input-field mt-1.5 w-full dark:bg-white/5 dark:border-white/10 dark:text-white" placeholder="https://api.example.com/v1" /></label>
      <div className="grid gap-3 sm:grid-cols-2"><label className="text-xs text-gray-600 dark:text-gray-300">API Key<input type="password" value={custom.apiKey} onChange={e => setCustom({ ...custom, apiKey: e.target.value })} className="input-field mt-1.5 w-full dark:bg-white/5 dark:border-white/10 dark:text-white" placeholder={customId ? '留空表示不修改' : 'sk-...'} /></label><label className="text-xs text-gray-600 dark:text-gray-300">默认模型<input value={custom.defaultModel} onChange={e => setCustom({ ...custom, defaultModel: e.target.value })} className="input-field mt-1.5 w-full dark:bg-white/5 dark:border-white/10 dark:text-white" placeholder="模型 ID" /></label></div>
      <button onClick={() => setAdvanced(value => !value)} className="flex items-center gap-1 text-xs font-medium text-purple-600 dark:text-purple-300">{advanced ? <ChevronDown size={14} /> : <ChevronRight size={14} />}高级设置</button>
      {advanced && <div className="space-y-3 rounded-xl bg-gray-50 p-4 dark:bg-white/[0.03]"><label className="block text-xs text-gray-600 dark:text-gray-300">模型列表（每行 ID|显示名称）<textarea value={modelsDraft} onChange={e => setModelsDraft(e.target.value)} rows={5} className="input-field mt-1.5 w-full font-mono text-xs dark:bg-white/5 dark:border-white/10 dark:text-white" placeholder={'gpt-4.1|GPT-4.1\ngpt-4.1-mini|GPT-4.1 Mini'} /></label><label className="flex items-center gap-2 text-xs text-gray-600 dark:text-gray-300"><input type="checkbox" checked={!!custom.useProxy} onChange={e => setCustom({ ...custom, useProxy: e.target.checked || undefined })} />通过本地代理访问</label><p className="text-[11px] text-gray-400">模型能力、上下文窗口和请求参数可在保存后由服务端模型配置继续维护。</p></div>}
      <div className="flex justify-end gap-2 border-t border-gray-100 pt-4 dark:border-white/5"><button onClick={() => setCustom(null)} className="rounded-lg border px-4 py-2 text-sm dark:border-white/10">取消</button><button onClick={() => void saveCustom()} disabled={busy} className="rounded-lg bg-purple-500 px-4 py-2 text-sm text-white disabled:opacity-50">保存</button></div>
    </div></Modal>}

    {authProvider && <Modal title={`登录 ${authProvider.name}`} onClose={() => void closeAuth()}><div className="space-y-4 p-5">
      {authProvider.kind === 'openai-codex' && !authFlow && <div className="grid grid-cols-2 gap-2"><button onClick={() => setAuthMethod('browser')} className={`rounded-xl border p-3 text-sm ${authMethod === 'browser' ? 'border-purple-500 bg-purple-500/5 text-purple-600' : 'border-gray-200 dark:border-white/10'}`}>浏览器登录</button><button onClick={() => setAuthMethod('device')} className={`rounded-xl border p-3 text-sm ${authMethod === 'device' ? 'border-purple-500 bg-purple-500/5 text-purple-600' : 'border-gray-200 dark:border-white/10'}`}>设备码登录</button></div>}
      {!authFlow && <><p className="text-sm text-gray-600 dark:text-gray-300">{authMethod === 'browser' ? '将在浏览器中完成授权。回调失败时可粘贴跳转地址或授权码。' : '生成设备码后，在登录页面完成授权，本页会自动刷新。'}</p><button onClick={() => void beginAuth()} disabled={busy} className="w-full rounded-lg bg-purple-500 py-2.5 text-sm text-white disabled:opacity-50">开始登录</button></>}
      {authFlow && <div className="space-y-4">
        {authFlow.status === 'connected' ? <div className="flex items-center gap-2 rounded-xl bg-emerald-500/10 p-4 text-sm text-emerald-600"><CheckCircle2 size={18} />登录成功</div> : authFlow.status === 'error' || authFlow.status === 'expired' ? <div className="rounded-xl bg-red-500/10 p-4 text-sm text-red-600"><AlertCircle size={18} className="mr-2 inline" />{authFlow.error || '授权已过期，请重试'}</div> : <div className="rounded-xl bg-purple-500/5 p-4 text-center"><p className="text-xs text-gray-500">请在登录页面输入验证码</p>{authFlow.userCode && <button onClick={() => void navigator.clipboard.writeText(authFlow.userCode!)} className="mt-2 font-mono text-2xl font-semibold tracking-widest text-gray-900 dark:text-white">{authFlow.userCode}<Copy size={14} className="ml-2 inline text-gray-400" /></button>}{authFlow.verificationUri && <a href={authFlow.verificationUri} target="_blank" rel="noreferrer" className="mt-3 flex items-center justify-center gap-1 text-xs text-purple-600">打开登录页面<ExternalLink size={12} /></a>}<p className="mt-3 flex items-center justify-center gap-2 text-xs text-gray-400"><Loader2 size={13} className="animate-spin" />等待授权</p></div>}
        {authProvider.kind === 'openai-codex' && authFlow.status === 'pending' && <div className="space-y-2"><label className="block text-xs text-gray-500">浏览器没有自动返回？粘贴回调地址或授权码<input value={manualCode} onChange={e => setManualCode(e.target.value)} className="input-field mt-1.5 w-full dark:bg-white/5 dark:border-white/10 dark:text-white" /></label><button disabled={!manualCode.trim()} onClick={() => void perform(async () => setAuthFlow(await completeProviderAuth(authProvider.id, authFlow.flowId, manualCode)))} className="rounded-lg border px-3 py-1.5 text-xs disabled:opacity-40 dark:border-white/10">提交</button></div>}
        {(authFlow.status === 'error' || authFlow.status === 'expired') && <button onClick={() => setAuthFlow(undefined)} className="w-full rounded-lg bg-purple-500 py-2 text-sm text-white">重新登录</button>}
      </div>}
    </div></Modal>}
  </div>;
}
