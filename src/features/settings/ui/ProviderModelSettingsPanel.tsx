import { useEffect, useMemo, useRef, useState } from 'react';
import { Brain, Check, ChevronDown, ChevronRight, Eye, EyeOff, Plus, RefreshCw, RotateCcw, Save, Trash2 } from 'lucide-react';
import {
  checkModelUpdates,
  THINKING_LEVEL_LABELS,
  updateProviderModels,
  useProviderStore,
  type ModelEntry,
  type ProviderView,
  type ThinkingLevel,
} from '@/core/config';
import { showToast } from '@/shell';
import { applyModelTokenLimits, applyModelVisionCapability, applyThinkingLevelsToModels } from './providerModelThinking';

interface ModelDraft {
  models: ModelEntry[];
  imageModels: ModelEntry[];
  defaultModel: string;
}

function cloneModel(model: ModelEntry): ModelEntry {
  return {
    ...model,
    capabilities: model.capabilities ? { ...model.capabilities } : undefined,
    thinkingLevelMap: model.thinkingLevelMap ? { ...model.thinkingLevelMap } : undefined,
  };
}

const CONFIGURABLE_THINKING_LEVELS = ['minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra'] as const;

function configuredThinkingLevels(model: ModelEntry): ThinkingLevel[] {
  if (!model.capabilities?.thinking) return [];
  const configured = model.thinkingLevelMap;
  if (!configured || Object.keys(configured).length === 0) return ['medium'];
  return CONFIGURABLE_THINKING_LEVELS.filter(level => configured[level] !== null && Object.prototype.hasOwnProperty.call(configured, level));
}

function fromProvider(provider: ProviderView): ModelDraft {
  return {
    models: provider.models.map(cloneModel),
    imageModels: provider.imageModels.map(cloneModel),
    defaultModel: provider.defaultModel ?? provider.models.find(model => model.enabled !== false)?.id ?? '',
  };
}

function sameDraft(provider: ProviderView, draft: ModelDraft): boolean {
  return draft.defaultModel === provider.defaultModel
    && JSON.stringify(draft.models) === JSON.stringify(provider.models)
    && JSON.stringify(draft.imageModels) === JSON.stringify(provider.imageModels);
}

export default function ProviderModelSettingsPanel() {
  const { providers, catalog, loading } = useProviderStore();
  const [selected, setSelected] = useState('all');
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [drafts, setDrafts] = useState<Record<string, ModelDraft>>({});
  const [newModels, setNewModels] = useState<Record<string, { id: string; name: string }>>({});
  const [advancedProviders, setAdvancedProviders] = useState<Set<string>>(new Set());
  const [thinkingSelections, setThinkingSelections] = useState<Record<string, Set<string>>>({});
  const [thinkingLevels, setThinkingLevels] = useState<Record<string, ThinkingLevel[]>>({});
  const [tokenLimits, setTokenLimits] = useState<Record<string, { contextWindow: string; maxOutput: string }>>({});
  const [saving, setSaving] = useState<string>();
  const addInputRefs = useRef<Record<string, HTMLInputElement | null>>({});

  useEffect(() => {
    setDrafts(current => Object.fromEntries(providers.map(provider => [
      provider.id,
      current[provider.id] && !sameDraft(provider, current[provider.id]) ? current[provider.id] : fromProvider(provider),
    ])));
    setExpanded(current => current.size === 0 ? new Set(providers.map(provider => provider.id)) : current);
  }, [providers]);

  const visibleProviders = selected === 'all' ? providers : providers.filter(provider => provider.id === selected);
  const totalModels = providers.reduce((sum, provider) => sum + (drafts[provider.id]?.models.length ?? provider.models.length), 0);
  const enabledModels = providers.reduce((sum, provider) => sum + (drafts[provider.id]?.models ?? provider.models).filter(model => model.enabled !== false).length, 0);
  const dirtyIds = useMemo(() => new Set(providers.filter(provider => drafts[provider.id] && !sameDraft(provider, drafts[provider.id])).map(provider => provider.id)), [drafts, providers]);

  const changeDraft = (id: string, update: (draft: ModelDraft) => ModelDraft) => {
    setDrafts(current => ({ ...current, [id]: update(current[id] ?? fromProvider(providers.find(provider => provider.id === id)!)) }));
  };

  const persistDraft = async (provider: ProviderView, draft: ModelDraft, successMessage: string) => {
    const enabled = draft.models.filter(model => model.enabled !== false);
    if (enabled.length === 0) { showToast({ message: `${provider.name} 至少需要启用一个模型`, type: 'error' }); return false; }
    if (!enabled.some(model => model.id === draft.defaultModel)) { showToast({ message: `${provider.name} 的默认模型必须处于启用状态`, type: 'error' }); return false; }
    setSaving(provider.id);
    try {
      await updateProviderModels(provider.id, draft.models, draft.imageModels, draft.defaultModel);
      showToast({ message: successMessage, type: 'success' });
      return true;
    } catch (error) {
      showToast({ message: error instanceof Error ? error.message : '保存失败', type: 'error' });
      return false;
    } finally { setSaving(undefined); }
  };

  const save = async (provider: ProviderView) => {
    const draft = drafts[provider.id] ?? fromProvider(provider);
    await persistDraft(provider, draft, `${provider.name} 模型配置已保存`);
  };

  const addModel = (provider: ProviderView) => {
    const input = newModels[provider.id] ?? { id: '', name: '' };
    const id = input.id.trim();
    if (!id) { showToast({ message: '请填写模型 ID', type: 'error' }); return; }
    const draft = drafts[provider.id] ?? fromProvider(provider);
    if (draft.models.some(model => model.id === id)) { showToast({ message: `模型 ${id} 已存在`, type: 'error' }); return; }
    changeDraft(provider.id, current => ({ ...current, models: [...current.models, { id, name: input.name.trim() || id, enabled: true }], defaultModel: current.defaultModel || id }));
    setNewModels(current => ({ ...current, [provider.id]: { id: '', name: '' } }));
  };

  const applyBatchThinking = async (provider: ProviderView) => {
    const selectedIds = thinkingSelections[provider.id] ?? new Set<string>();
    if (selectedIds.size === 0) {
      showToast({ message: '请至少选择一个模型', type: 'error' });
      return;
    }
    const levels = thinkingLevels[provider.id] ?? [];
    const current = drafts[provider.id] ?? fromProvider(provider);
    const next = {
      ...current,
      models: applyThinkingLevelsToModels(current.models, selectedIds, levels),
    };
    changeDraft(provider.id, () => next);
    await persistDraft(
      provider,
      next,
      levels.length > 0
        ? `已保存 ${selectedIds.size} 个模型的思考强度`
        : `已关闭并保存 ${selectedIds.size} 个模型的思考能力`,
    );
  };

  const applyBatchTokenLimits = async (provider: ProviderView) => {
    const selectedIds = thinkingSelections[provider.id] ?? new Set<string>();
    if (selectedIds.size === 0) {
      showToast({ message: '请至少选择一个模型', type: 'error' });
      return;
    }
    const draft = tokenLimits[provider.id] ?? { contextWindow: '', maxOutput: '' };
    const contextWindow = draft.contextWindow.trim() ? Number(draft.contextWindow) : undefined;
    const maxOutput = draft.maxOutput.trim() ? Number(draft.maxOutput) : undefined;
    if (contextWindow === undefined && maxOutput === undefined) {
      showToast({ message: '请填写上下文窗口或最大输出', type: 'error' });
      return;
    }
    if (contextWindow !== undefined && (!Number.isSafeInteger(contextWindow) || contextWindow <= 0)) {
      showToast({ message: '上下文窗口必须是正整数', type: 'error' });
      return;
    }
    if (maxOutput !== undefined && (!Number.isSafeInteger(maxOutput) || maxOutput <= 0 || maxOutput > 4_294_967_295)) {
      showToast({ message: '最大输出必须是有效的正整数', type: 'error' });
      return;
    }
    if (contextWindow !== undefined && maxOutput !== undefined && maxOutput > contextWindow) {
      showToast({ message: '最大输出不能大于上下文窗口', type: 'error' });
      return;
    }
    const current = drafts[provider.id] ?? fromProvider(provider);
    const next = {
      ...current,
      models: applyModelTokenLimits(current.models, selectedIds, { contextWindow, maxOutput }),
    };
    changeDraft(provider.id, () => next);
    await persistDraft(provider, next, `已保存 ${selectedIds.size} 个模型的上下文配置`);
  };

  const applyBatchVision = async (provider: ProviderView, supported: boolean) => {
    const selectedIds = thinkingSelections[provider.id] ?? new Set<string>();
    if (selectedIds.size === 0) {
      showToast({ message: '请至少选择一个模型', type: 'error' });
      return;
    }
    const current = drafts[provider.id] ?? fromProvider(provider);
    const next = {
      ...current,
      models: applyModelVisionCapability(current.models, selectedIds, supported),
    };
    changeDraft(provider.id, () => next);
    await persistDraft(provider, next, `已将 ${selectedIds.size} 个模型保存为${supported ? '支持' : '不支持'}视觉输入`);
  };

  return <div className="space-y-5">
    <div className="flex items-start justify-between gap-4">
      <div><h3 className="text-base font-semibold text-gray-900 dark:text-white">模型目录</h3><p className="mt-1 max-w-2xl text-xs leading-5 text-gray-500">按 Provider 管理可见模型和默认模型。关闭的模型不会出现在模型选择器；仍可通过准确模型 ID 添加账号实际可用、但目录尚未收录的模型。</p></div>
      <div className="flex shrink-0 items-center gap-2"><button onClick={() => void checkModelUpdates()} className="rounded-lg border border-gray-200 px-3 py-1.5 text-xs text-gray-600 hover:bg-gray-50 dark:border-white/10 dark:text-gray-300 dark:hover:bg-white/5" title="对比全部已发现模型（含此前跳过的），重新选择采纳"><RefreshCw size={13} className="mr-1 inline" />检查模型更新</button><div className="rounded-full bg-purple-500/10 px-3 py-1.5 text-xs font-medium text-purple-600 dark:text-purple-300">{enabledModels}/{totalModels} 可见</div></div>
    </div>

    {loading && providers.length === 0 ? <div className="py-12 text-center text-sm text-gray-400">正在加载模型目录…</div> : providers.length === 0 ? <div className="rounded-xl border border-dashed border-gray-300 py-12 text-center text-sm text-gray-400 dark:border-white/15">请先在「AI Provider」中连接服务</div> : <div className="grid min-h-[360px] gap-5 md:grid-cols-[190px_minmax(0,1fr)]">
      <aside className="space-y-1 border-r border-gray-200 pr-4 dark:border-white/10">
        <button onClick={() => setSelected('all')} className={`w-full rounded-lg px-3 py-2.5 text-left ${selected === 'all' ? 'bg-purple-500/10 text-purple-700 dark:text-purple-300' : 'text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-white/5'}`}><span className="block text-sm font-medium">所有 Provider</span><span className="mt-0.5 block text-[11px] opacity-65">{enabledModels}/{totalModels} 可见</span></button>
        {providers.map(provider => { const models = drafts[provider.id]?.models ?? provider.models; return <button key={provider.id} onClick={() => { setSelected(provider.id); setExpanded(current => new Set(current).add(provider.id)); }} className={`w-full rounded-lg px-3 py-2.5 text-left ${selected === provider.id ? 'bg-purple-500/10 text-purple-700 dark:text-purple-300' : 'text-gray-600 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-white/5'}`}><span className="block truncate text-sm font-medium">{provider.name}</span><span className="mt-0.5 block text-[11px] opacity-65">{models.filter(model => model.enabled !== false).length}/{models.length} 可见</span></button>; })}
      </aside>

      <div className="min-w-0 space-y-3">
        <div className="flex gap-2"><button onClick={() => setExpanded(new Set(visibleProviders.map(provider => provider.id)))} className="rounded-lg border border-gray-200 px-3 py-1.5 text-xs text-gray-600 hover:bg-gray-50 dark:border-white/10 dark:text-gray-300 dark:hover:bg-white/5">全部展开</button><button onClick={() => setExpanded(new Set())} className="rounded-lg border border-gray-200 px-3 py-1.5 text-xs text-gray-600 hover:bg-gray-50 dark:border-white/10 dark:text-gray-300 dark:hover:bg-white/5">全部折叠</button></div>
        {visibleProviders.map(provider => {
          const draft = drafts[provider.id] ?? fromProvider(provider);
          const isExpanded = expanded.has(provider.id);
          const add = newModels[provider.id] ?? { id: '', name: '' };
          const catalogModelIds = new Set(catalog.find(entry => entry.kind === provider.kind)?.models.map(model => model.id) ?? []);
          const retiredModelIds = new Set(provider.modelChanges?.removed.map(model => model.id) ?? []);
          return <section key={provider.id} className="overflow-hidden rounded-xl border border-gray-200 bg-white dark:border-white/10 dark:bg-white/[0.02]">
            <div className="flex items-center gap-3 px-4 py-3">
              <button onClick={() => setExpanded(current => { const next = new Set(current); if (next.has(provider.id)) next.delete(provider.id); else next.add(provider.id); return next; })} className="rounded p-1 text-gray-500 hover:bg-gray-100 dark:hover:bg-white/10">{isExpanded ? <ChevronDown size={16} /> : <ChevronRight size={16} />}</button>
              <div className="min-w-0 flex-1"><div className="flex items-center gap-2"><strong className="truncate text-sm text-gray-900 dark:text-white">{provider.name}</strong>{provider.builtin && <span className="rounded bg-gray-100 px-1.5 py-0.5 text-[10px] text-gray-500 dark:bg-white/10">内置</span>}{dirtyIds.has(provider.id) && <span className="text-[10px] text-amber-600">未保存</span>}</div><p className="text-[11px] text-gray-400">{draft.models.filter(model => model.enabled !== false).length}/{draft.models.length} 可见 · 默认 {draft.defaultModel || '未设置'}</p></div>
              <button onClick={() => changeDraft(provider.id, current => ({ ...current, models: current.models.map(model => ({ ...model, enabled: true })) }))} className="rounded-lg border border-gray-200 px-2.5 py-1.5 text-xs text-gray-600 dark:border-white/10 dark:text-gray-300">全部启用</button>
              <button onClick={() => setAdvancedProviders(current => { const next = new Set(current); if (next.has(provider.id)) next.delete(provider.id); else next.add(provider.id); return next; })} className={`rounded-lg border px-2.5 py-1.5 text-xs ${advancedProviders.has(provider.id) ? 'border-purple-300 bg-purple-500/10 text-purple-600 dark:border-purple-500/40 dark:text-purple-300' : 'border-gray-200 text-gray-600 dark:border-white/10 dark:text-gray-300'}`} aria-expanded={advancedProviders.has(provider.id)}><Brain size={13} className="mr-1 inline" />高级配置</button>
              <button onClick={() => setExpanded(current => { const next = new Set(current); next.add(provider.id); setTimeout(() => addInputRefs.current[provider.id]?.focus(), 60); return next; })} className="rounded-lg border border-gray-200 p-1.5 text-gray-500 dark:border-white/10" aria-label={`添加 ${provider.name} 模型`}><Plus size={14} /></button>
              {dirtyIds.has(provider.id) && <><button onClick={() => setDrafts(current => ({ ...current, [provider.id]: fromProvider(provider) }))} className="rounded-lg p-1.5 text-gray-400 hover:bg-gray-100 dark:hover:bg-white/10" aria-label="放弃修改"><RotateCcw size={14} /></button><button onClick={() => void save(provider)} disabled={saving === provider.id} className="rounded-lg bg-purple-500 px-3 py-1.5 text-xs text-white disabled:opacity-50"><Save size={13} className="mr-1 inline" />保存</button></>}
            </div>
            {isExpanded && <div className="border-t border-gray-100 px-4 py-3 dark:border-white/5">
              {advancedProviders.has(provider.id) && <div className="mb-3 rounded-xl border border-purple-200 bg-purple-50/60 p-4 dark:border-purple-500/20 dark:bg-purple-500/[0.06]">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div><p className="text-xs font-semibold text-gray-800 dark:text-gray-100">批量高级配置</p><p className="mt-1 text-[11px] leading-5 text-gray-500 dark:text-gray-400">先选择模型，再分别应用思考强度或上下文参数。所有数值应以供应商模型文档为准。</p></div>
                  <div className="flex gap-2"><button type="button" onClick={() => setThinkingSelections(current => ({ ...current, [provider.id]: new Set(draft.models.filter(model => model.enabled !== false).map(model => model.id)) }))} className="rounded-lg border border-purple-200 px-2.5 py-1.5 text-[11px] text-purple-700 dark:border-purple-500/30 dark:text-purple-300">选择全部可见模型</button><button type="button" onClick={() => setThinkingSelections(current => ({ ...current, [provider.id]: new Set() }))} className="rounded-lg px-2.5 py-1.5 text-[11px] text-gray-500 hover:bg-white/70 dark:text-gray-400 dark:hover:bg-white/5">清空</button></div>
                </div>
                <fieldset className="mt-3"><legend className="text-[11px] font-medium text-gray-600 dark:text-gray-300">应用到模型</legend><div className="mt-2 grid gap-2 sm:grid-cols-2">{draft.models.map(model => { const checked = thinkingSelections[provider.id]?.has(model.id) ?? false; return <label key={model.id} className={`flex min-h-11 cursor-pointer items-center gap-2 rounded-lg border px-3 py-2 text-xs ${checked ? 'border-purple-300 bg-white text-purple-700 dark:border-purple-500/40 dark:bg-white/[0.06] dark:text-purple-300' : 'border-gray-200/80 bg-white/50 text-gray-600 dark:border-white/10 dark:bg-transparent dark:text-gray-300'}`}><input type="checkbox" checked={checked} onChange={event => setThinkingSelections(current => { const next = new Set(current[provider.id] ?? []); if (event.target.checked) next.add(model.id); else next.delete(model.id); return { ...current, [provider.id]: next }; })} className="accent-purple-600" /><span className="min-w-0 truncate font-mono">{model.id}</span></label>; })}</div></fieldset>
                <fieldset className="mt-4"><legend className="text-[11px] font-medium text-gray-600 dark:text-gray-300">支持的思考强度</legend><div className="mt-2 flex flex-wrap gap-2">{CONFIGURABLE_THINKING_LEVELS.map(level => { const checked = thinkingLevels[provider.id]?.includes(level) ?? false; return <label key={level} className={`flex min-h-10 cursor-pointer items-center gap-2 rounded-lg border px-3 py-2 text-xs ${checked ? 'border-amber-300 bg-amber-50 text-amber-700 dark:border-amber-500/40 dark:bg-amber-500/10 dark:text-amber-300' : 'border-gray-200 bg-white text-gray-500 dark:border-white/10 dark:bg-white/[0.02] dark:text-gray-400'}`}><input type="checkbox" checked={checked} onChange={event => setThinkingLevels(current => { const next = new Set(current[provider.id] ?? []); if (event.target.checked) next.add(level); else next.delete(level); return { ...current, [provider.id]: CONFIGURABLE_THINKING_LEVELS.filter(item => next.has(item)) }; })} className="accent-amber-600" />{THINKING_LEVEL_LABELS[level]}</label>; })}</div><div className="mt-3 flex items-center justify-between gap-3"><p className="text-[10px] leading-5 text-gray-400">不选择任何强度并保存，会把所选模型标记为不支持思考。</p><button type="button" disabled={saving === provider.id} onClick={() => void applyBatchThinking(provider)} className="min-h-10 shrink-0 rounded-lg border border-amber-300 bg-white px-4 py-2 text-xs font-medium text-amber-700 hover:bg-amber-50 disabled:opacity-50 dark:border-amber-500/40 dark:bg-white/[0.03] dark:text-amber-300">{saving === provider.id ? '保存中…' : '保存思考强度'}</button></div></fieldset>
                <fieldset className="mt-4 border-t border-purple-200 pt-4 dark:border-purple-500/20"><legend className="px-1 text-[11px] font-medium text-gray-600 dark:text-gray-300">视觉输入能力</legend><div className="mt-2 flex flex-wrap items-center justify-between gap-3"><p className="max-w-md text-[10px] leading-5 text-gray-400">设置模型是否接受图片作为对话输入。此项不代表生图能力。</p><div className="flex flex-wrap gap-2"><button type="button" disabled={saving === provider.id} onClick={() => void applyBatchVision(provider, false)} className="min-h-10 rounded-lg border border-gray-200 bg-white px-3 py-2 text-xs text-gray-600 hover:bg-gray-50 disabled:opacity-50 dark:border-white/10 dark:bg-white/[0.03] dark:text-gray-300"><EyeOff size={13} className="mr-1 inline" />不支持视觉</button><button type="button" disabled={saving === provider.id} onClick={() => void applyBatchVision(provider, true)} className="min-h-10 rounded-lg border border-blue-300 bg-blue-50 px-3 py-2 text-xs font-medium text-blue-700 hover:bg-blue-100 disabled:opacity-50 dark:border-blue-500/40 dark:bg-blue-500/10 dark:text-blue-300"><Eye size={13} className="mr-1 inline" />支持视觉</button></div></div></fieldset>
                <fieldset className="mt-4 border-t border-purple-200 pt-4 dark:border-purple-500/20"><legend className="px-1 text-[11px] font-medium text-gray-600 dark:text-gray-300">上下文与输出</legend><div className="mt-2 grid gap-3 sm:grid-cols-2"><label className="text-[11px] text-gray-600 dark:text-gray-300">上下文窗口（tokens）<input type="number" inputMode="numeric" min="1" step="1" value={tokenLimits[provider.id]?.contextWindow ?? ''} onChange={event => setTokenLimits(current => ({ ...current, [provider.id]: { contextWindow: event.target.value, maxOutput: current[provider.id]?.maxOutput ?? '' } }))} className="input-field mt-1.5 min-h-11 w-full font-mono text-xs dark:border-white/10 dark:bg-white/5 dark:text-white" placeholder="例如 128000" /></label><label className="text-[11px] text-gray-600 dark:text-gray-300">最大输出（tokens）<input type="number" inputMode="numeric" min="1" step="1" value={tokenLimits[provider.id]?.maxOutput ?? ''} onChange={event => setTokenLimits(current => ({ ...current, [provider.id]: { contextWindow: current[provider.id]?.contextWindow ?? '', maxOutput: event.target.value } }))} className="input-field mt-1.5 min-h-11 w-full font-mono text-xs dark:border-white/10 dark:bg-white/5 dark:text-white" placeholder="例如 8192" /></label></div><div className="mt-3 flex items-center justify-between gap-3"><p className="text-[10px] leading-5 text-gray-400">留空的字段保持原值；最大输出不能超过上下文窗口。</p><button type="button" disabled={saving === provider.id} onClick={() => void applyBatchTokenLimits(provider)} className="min-h-10 shrink-0 rounded-lg bg-purple-600 px-4 py-2 text-xs font-medium text-white hover:bg-purple-700 disabled:opacity-50">{saving === provider.id ? '保存中…' : '保存上下文配置'}</button></div></fieldset>
                <p className="mt-4 text-[11px] text-gray-500">已选择 {thinkingSelections[provider.id]?.size ?? 0} 个模型。点击对应保存按钮后立即生效，无需再次确认。</p>
              </div>}
              <div className="divide-y divide-gray-100 dark:divide-white/5">{draft.models.map(model => <div key={model.id} className="flex items-center gap-3 py-3">
                <label className="relative inline-flex cursor-pointer items-center"><input type="checkbox" className="peer sr-only" checked={model.enabled !== false} onChange={event => changeDraft(provider.id, current => ({ ...current, models: current.models.map(item => item.id === model.id ? { ...item, enabled: event.target.checked } : item) }))} /><span className="h-5 w-9 rounded-full bg-gray-200 transition peer-checked:bg-gray-900 after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-white after:transition peer-checked:after:translate-x-4 dark:bg-white/15 dark:peer-checked:bg-purple-500" /></label>
                <div className={`min-w-0 flex-1 ${model.enabled === false ? 'opacity-45' : ''}`}><div className="flex flex-wrap items-center gap-2"><span className="break-all font-mono text-xs text-gray-800 dark:text-gray-100">{model.id}</span>{catalogModelIds.has(model.id) && <span className="rounded bg-gray-100 px-1.5 py-0.5 text-[10px] text-gray-500 dark:bg-white/10">目录</span>}{retiredModelIds.has(model.id) && <span className="rounded bg-red-500/10 px-1.5 py-0.5 text-[10px] text-red-500">已下线</span>}{model.name && model.name !== model.id && <span className="text-[11px] text-gray-400">{model.name}</span>}{model.capabilities?.vision && <span className="rounded bg-blue-500/10 px-1.5 py-0.5 text-[10px] text-blue-600">视觉</span>}{model.capabilities?.thinking && <span className="rounded bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-600">思考：{configuredThinkingLevels(model).map(level => THINKING_LEVEL_LABELS[level]).join('/')}</span>}</div>{(model.contextWindow || model.maxOutput) && <p className="mt-1 text-[10px] text-gray-400">{model.contextWindow ? `上下文 ${model.contextWindow.toLocaleString()}` : ''}{model.contextWindow && model.maxOutput ? ' · ' : ''}{model.maxOutput ? `最大输出 ${model.maxOutput.toLocaleString()}` : ''}</p>}</div>
                <button disabled={model.enabled === false} onClick={() => changeDraft(provider.id, current => ({ ...current, defaultModel: model.id }))} className={`rounded-lg px-2.5 py-1 text-[11px] ${draft.defaultModel === model.id ? 'bg-purple-500/10 text-purple-600 dark:text-purple-300' : 'text-gray-400 hover:bg-gray-100 disabled:opacity-30 dark:hover:bg-white/10'}`}>{draft.defaultModel === model.id ? <><Check size={12} className="mr-1 inline" />默认</> : '设为默认'}</button>
                <button onClick={() => changeDraft(provider.id, current => ({ ...current, models: current.models.filter(item => item.id !== model.id) }))} className="rounded p-1.5 text-gray-300 hover:bg-red-500/10 hover:text-red-500" aria-label={`删除 ${model.id}`} title="删除后不再自动推荐，可随时重新添加"><Trash2 size={14} /></button>
              </div>)}</div>
              {draft.imageModels.length > 0 && <div className="mt-3 rounded-xl border border-pink-500/20 bg-pink-500/[0.04] p-3">
                <div className="flex flex-wrap items-center justify-between gap-2"><p className="text-xs font-medium text-pink-600 dark:text-pink-300">生图模型 · {draft.imageModels.filter(model => model.enabled !== false).length}/{draft.imageModels.length} 启用</p><p className="text-[10px] text-gray-400">自动联动生图工具，不在聊天模型选择器中显示</p></div>
                <div className="divide-y divide-pink-500/10">{draft.imageModels.map(model => <div key={model.id} className="flex items-center gap-3 py-2.5">
                  <label className="relative inline-flex cursor-pointer items-center"><input type="checkbox" className="peer sr-only" checked={model.enabled !== false} onChange={event => changeDraft(provider.id, current => ({ ...current, imageModels: current.imageModels.map(item => item.id === model.id ? { ...item, enabled: event.target.checked } : item) }))} /><span className="h-5 w-9 rounded-full bg-gray-200 transition peer-checked:bg-gray-900 after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-white after:transition peer-checked:after:translate-x-4 dark:bg-white/15 dark:peer-checked:bg-pink-500" /></label>
                  <div className={`min-w-0 flex-1 ${model.enabled === false ? 'opacity-45' : ''}`}><div className="flex flex-wrap items-center gap-2"><span className="break-all font-mono text-xs text-gray-800 dark:text-gray-100">{model.id}</span><span className="rounded bg-pink-500/10 px-1.5 py-0.5 text-[10px] text-pink-600 dark:text-pink-300">生图</span>{retiredModelIds.has(model.id) && <span className="rounded bg-red-500/10 px-1.5 py-0.5 text-[10px] text-red-500">已下线</span>}</div></div>
                  <button onClick={() => changeDraft(provider.id, current => ({ ...current, imageModels: current.imageModels.filter(item => item.id !== model.id) }))} className="rounded p-1.5 text-gray-300 hover:bg-red-500/10 hover:text-red-500" aria-label={`删除 ${model.id}`} title="删除后不再自动推荐，可随时重新添加"><Trash2 size={14} /></button>
                </div>)}</div>
              </div>}
              <div className="mt-3 grid gap-2 rounded-xl bg-gray-50 p-3 dark:bg-white/[0.03] sm:grid-cols-[1fr_1fr_auto]"><input ref={element => { addInputRefs.current[provider.id] = element; }} value={add.id} onChange={event => setNewModels(current => ({ ...current, [provider.id]: { ...add, id: event.target.value } }))} className="input-field w-full font-mono text-xs dark:border-white/10 dark:bg-white/5 dark:text-white" placeholder="模型 ID" /><input value={add.name} onChange={event => setNewModels(current => ({ ...current, [provider.id]: { ...add, name: event.target.value } }))} className="input-field w-full text-xs dark:border-white/10 dark:bg-white/5 dark:text-white" placeholder="显示名称（可选）" /><button onClick={() => addModel(provider)} className="rounded-lg bg-gray-900 px-3 py-2 text-xs text-white dark:bg-white dark:text-gray-900"><Plus size={13} className="mr-1 inline" />添加模型</button></div>
            </div>}
          </section>;
        })}
      </div>
    </div>}
  </div>;
}
