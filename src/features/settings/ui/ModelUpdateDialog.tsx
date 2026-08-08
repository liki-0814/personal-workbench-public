import { useEffect, useMemo, useState } from 'react';
import { Plus, Sparkles, Trash2, X } from 'lucide-react';
import {
  adoptModelChanges,
  dismissModelChanges,
  pruneModelChanges,
  useProviderStore,
  type ModelEntry,
  type ProviderView,
} from '@/core/config';

/**
 * Surfaces the gap between live model discovery and the user's
 * configuration: models a provider serves but the user has not adopted yet
 * (check to adopt), and configured models the provider retired (prune or
 * keep).  Closing the dialog dismisses what was shown ("not adopted means
 * not wanted"), so it never re-pops for the same models; a full diff is
 * only produced by the explicit "check for model updates" action.
 */
export default function ModelUpdateDialog({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { providers, fullDiffAt } = useProviderStore();
  const [busy, setBusy] = useState<string>();
  const [deferred, setDeferred] = useState(false);
  // Unchecked model ids per provider; absent means "all checked".
  const [unchecked, setUnchecked] = useState<Record<string, Set<string>>>({});

  // An explicit full-diff check re-opens the dialog.
  useEffect(() => {
    if (fullDiffAt) setDeferred(false);
  }, [fullDiffAt]);

  const changed = useMemo(
    () => providers.filter(provider =>
      (provider.modelChanges?.added.length ?? 0) > 0 || (provider.modelChanges?.removed.length ?? 0) > 0),
    [providers],
  );
  if (deferred || changed.length === 0) return null;

  const act = async (key: string, action: () => Promise<void>) => {
    setBusy(key);
    try { await action(); } finally { setBusy(undefined); }
  };

  // Closing without adopting records the visible delta as dismissed, so the
  // same models never trigger the dialog again.
  const defer = () => {
    setDeferred(true);
    for (const provider of changed) {
      void dismissModelChanges(provider.id).catch(() => undefined);
    }
  };

  return (
    <div className="fixed inset-0 z-[85] flex items-center justify-center bg-black/40 p-4 backdrop-blur-sm" onClick={defer}>
      <div className="max-h-[80vh] w-full max-w-lg overflow-y-auto rounded-2xl border border-gray-200 bg-white p-5 shadow-2xl dark:border-white/10 dark:bg-[#161B22]" onClick={event => event.stopPropagation()}>
        <div className="flex items-start justify-between gap-3">
          <div className="flex items-center gap-2">
            <Sparkles size={18} className="text-purple-500" />
            <h3 className="text-base font-semibold text-gray-900 dark:text-white">模型目录已更新</h3>
          </div>
          <button onClick={defer} className="rounded-lg p-1.5 text-gray-400 hover:bg-gray-100 dark:hover:bg-white/10" aria-label="知道了"><X size={16} /></button>
        </div>
        <p className="mt-1 text-xs leading-5 text-gray-500">运行时发现以下模型与你的配置存在差异，由你决定采纳哪些。勾选后添加并启用；已下线模型保留在配置中，可随时一键清理。直接关闭视为不采纳，不会再次提醒；可在模型目录中手动「检查模型更新」重新对比。</p>

        <div className="mt-4 space-y-4">
          {changed.map(provider => <ProviderChangeSection key={provider.id} provider={provider} busy={busy} act={act} unchecked={unchecked[provider.id]} onToggle={modelId => {
            setUnchecked(current => {
              const added = provider.modelChanges?.added ?? [];
              const next = new Set(current[provider.id] ?? []);
              if (next.has(modelId)) next.delete(modelId);
              else next.add(modelId);
              // Normalize "nothing unchecked" back to the absent state.
              if (next.size === 0) { const copy = { ...current }; delete copy[provider.id]; return copy; }
              if (added.every(model => next.has(model.id))) { const copy = { ...current }; delete copy[provider.id]; return copy; }
              return { ...current, [provider.id]: next };
            });
          }} />)}
        </div>

        <div className="mt-5 flex items-center justify-between gap-3">
          <button onClick={() => { defer(); onOpenSettings(); }} className="text-xs text-purple-600 hover:underline dark:text-purple-300">在设置中管理模型目录</button>
          <button onClick={defer} className="rounded-lg border border-gray-200 px-3 py-1.5 text-xs text-gray-600 hover:bg-gray-50 dark:border-white/10 dark:text-gray-300 dark:hover:bg-white/5">知道了</button>
        </div>
      </div>
    </div>
  );
}

function isImageGenModel(model: ModelEntry): boolean {
  return model.capabilities?.image === true || /image|imagine/i.test(model.id);
}

function ProviderChangeSection({ provider, busy, act, unchecked, onToggle }: {
  provider: ProviderView;
  busy?: string;
  act: (key: string, action: () => Promise<void>) => void;
  unchecked?: Set<string>;
  onToggle: (modelId: string) => void;
}) {
  const added = provider.modelChanges?.added ?? [];
  const removed = provider.modelChanges?.removed ?? [];
  const selected = added.filter(model => !unchecked?.has(model.id)).map(model => model.id);
  return (
    <section className="rounded-xl border border-gray-200 p-4 dark:border-white/10">
      <strong className="text-sm text-gray-900 dark:text-white">{provider.name}</strong>

      {added.length > 0 && <div className="mt-3">
        <p className="text-xs font-medium text-emerald-600 dark:text-emerald-400">新发现 {added.length} 个模型，勾选要添加的</p>
        <ul className="mt-1.5 max-h-44 space-y-1 overflow-y-auto">
          {added.map(model => <li key={model.id} className="flex items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-gray-50 dark:hover:bg-white/5">
            <input type="checkbox" checked={!unchecked?.has(model.id)} onChange={() => onToggle(model.id)} className="h-3.5 w-3.5 accent-purple-500" aria-label={`添加 ${model.id}`} />
            <span className="break-all font-mono text-[11px] text-gray-700 dark:text-gray-200">{model.id}</span>
            {isImageGenModel(model) && <span className="shrink-0 rounded bg-pink-500/10 px-1.5 py-0.5 text-[10px] text-pink-600 dark:text-pink-300">生图</span>}
          </li>)}
        </ul>
        <div className="mt-2.5 flex gap-2">
          <button disabled={busy !== undefined || selected.length === 0} onClick={() => void act(`${provider.id}:adopt`, () => adoptModelChanges(provider.id, selected))} className="rounded-lg bg-purple-500 px-3 py-1.5 text-xs text-white disabled:opacity-50"><Plus size={12} className="mr-1 inline" />添加选中（{selected.length}）</button>
          <button disabled={busy !== undefined} onClick={() => void act(`${provider.id}:dismiss-added`, () => dismissModelChanges(provider.id, { added: true }))} className="rounded-lg border border-gray-200 px-3 py-1.5 text-xs text-gray-600 disabled:opacity-50 dark:border-white/10 dark:text-gray-300">暂不添加</button>
        </div>
      </div>}

      {removed.length > 0 && <div className="mt-3">
        <p className="text-xs font-medium text-red-500">不再提供 {removed.length} 个模型</p>
        <ul className="mt-1.5 flex flex-wrap gap-1.5">
          {removed.map(model => <li key={model.id} className="rounded bg-red-500/10 px-2 py-0.5 font-mono text-[11px] text-red-600 line-through dark:text-red-400">{model.id}</li>)}
        </ul>
        <div className="mt-2.5 flex gap-2">
          <button disabled={busy !== undefined} onClick={() => void act(`${provider.id}:prune`, () => pruneModelChanges(provider.id))} className="rounded-lg bg-red-500 px-3 py-1.5 text-xs text-white disabled:opacity-50"><Trash2 size={12} className="mr-1 inline" />清理下线模型</button>
          <button disabled={busy !== undefined} onClick={() => void act(`${provider.id}:dismiss-removed`, () => dismissModelChanges(provider.id, { removed: true }))} className="rounded-lg border border-gray-200 px-3 py-1.5 text-xs text-gray-600 disabled:opacity-50 dark:border-white/10 dark:text-gray-300">保留</button>
        </div>
      </div>}
    </section>
  );
}
