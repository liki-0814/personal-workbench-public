import { useCallback, useEffect, useState } from 'react';
import {
  AlertCircle,
  Brain,
  ChevronDown,
  ChevronRight,
  Loader2,
  Plus,
  RefreshCw,
  Save,
  Trash2,
} from 'lucide-react';
import { showToast } from '@/shell';
import { saveLocalConfig, useLocalConfig } from '@/core/config';
import {
  createMemoryEntry,
  deleteMemoryEntry,
  fetchMemoryEntry,
  fetchMemoryOverview,
  isValidMemorySlug,
  updateMemoryEntry,
  updateMemoryIndex,
  updateMemoryProfile,
  type MemoryOverview,
} from '../api';

type SubTab = 'profile' | 'facts';

function formatTs(ts: number): string {
  if (!ts) return '—';
  return new Date(ts * 1000).toLocaleString();
}

function parseApiError(err: unknown): string {
  if (!(err instanceof Error)) return '操作失败';
  const m = err.message.match(/\{.*"error"\s*:\s*"([^"]+)"/);
  if (m?.[1]) return m[1];
  return err.message.replace(/^API \w+ \S+ failed \(\d+\):\s*/, '') || '操作失败';
}

export default function MemorySettingsPanel() {
  const localConfig = useLocalConfig();
  const [subTab, setSubTab] = useState<SubTab>('profile');
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [overview, setOverview] = useState<MemoryOverview | null>(null);
  const [dreamProject, setDreamProject] = useState(localConfig.memory?.dreamProject ?? '');

  const [profileDraft, setProfileDraft] = useState('');
  const [profileDirty, setProfileDirty] = useState(false);

  const [selectedSlug, setSelectedSlug] = useState<string | null>(null);
  const [entryLoading, setEntryLoading] = useState(false);
  const [isNewEntry, setIsNewEntry] = useState(false);
  const [newSlug, setNewSlug] = useState('');
  const [summaryDraft, setSummaryDraft] = useState('');
  const [contentDraft, setContentDraft] = useState('');
  const [entryDirty, setEntryDirty] = useState(false);

  const [showIndexRaw, setShowIndexRaw] = useState(false);
  const [indexDraft, setIndexDraft] = useState('');
  const [indexDirty, setIndexDirty] = useState(false);

  const loadOverview = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await fetchMemoryOverview();
      setOverview(data);
      setProfileDraft(data.profile);
      setProfileDirty(false);
      setIndexDraft(data.index_raw);
      setIndexDirty(false);
    } catch (e) {
      setError(parseApiError(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadOverview();
  }, [loadOverview]);

  const loadEntry = useCallback(async (slug: string) => {
    setEntryLoading(true);
    try {
      const entry = await fetchMemoryEntry(slug);
      setSummaryDraft(entry.summary);
      setContentDraft(entry.content);
      setEntryDirty(false);
    } catch (e) {
      showToast({ message: parseApiError(e), type: 'error' });
    } finally {
      setEntryLoading(false);
    }
  }, []);

  const handleSelectEntry = (slug: string) => {
    setIsNewEntry(false);
    setSelectedSlug(slug);
    setNewSlug('');
    void loadEntry(slug);
  };

  const handleStartNewEntry = () => {
    setIsNewEntry(true);
    setSelectedSlug(null);
    setNewSlug('');
    setSummaryDraft('');
    setContentDraft('');
    setEntryDirty(false);
  };

  const handleSaveProfile = async () => {
    setSaving(true);
    try {
      await updateMemoryProfile(profileDraft);
      setProfileDirty(false);
      showToast({ message: '用户画像已保存', type: 'success' });
      const data = await fetchMemoryOverview();
      setOverview(data);
    } catch (e) {
      showToast({ message: parseApiError(e), type: 'error' });
    } finally {
      setSaving(false);
    }
  };

  const handleSaveEntry = async () => {
    if (isNewEntry) {
      if (!isValidMemorySlug(newSlug.trim())) {
        showToast({ message: 'slug 仅允许字母、数字、_、-，最长 64 字符', type: 'error' });
        return;
      }
      if (!summaryDraft.trim()) {
        showToast({ message: '请填写摘要', type: 'error' });
        return;
      }
      setSaving(true);
      try {
        const res = await createMemoryEntry({
          slug: newSlug.trim(),
          summary: summaryDraft.trim(),
          content: contentDraft,
        });
        if (res.slug !== res.requested_slug) {
          showToast({
            message: `slug 冲突，已保存为 ${res.slug}`,
            type: 'success',
          });
        } else {
          showToast({ message: '条目已创建', type: 'success' });
        }
        setIsNewEntry(false);
        setSelectedSlug(res.slug);
        setEntryDirty(false);
        const data = await fetchMemoryOverview();
        setOverview(data);
        await loadEntry(res.slug);
      } catch (e) {
        showToast({ message: parseApiError(e), type: 'error' });
      } finally {
        setSaving(false);
      }
      return;
    }

    if (!selectedSlug) return;
    if (!summaryDraft.trim()) {
      showToast({ message: '请填写摘要', type: 'error' });
      return;
    }
    setSaving(true);
    try {
      await updateMemoryEntry(selectedSlug, {
        summary: summaryDraft.trim(),
        content: contentDraft,
      });
      setEntryDirty(false);
      showToast({ message: '条目已保存', type: 'success' });
      const data = await fetchMemoryOverview();
      setOverview(data);
    } catch (e) {
      showToast({ message: parseApiError(e), type: 'error' });
    } finally {
      setSaving(false);
    }
  };

  const handleDeleteEntry = async () => {
    if (!selectedSlug || isNewEntry) return;
    const ok = window.confirm(`确定删除记忆条目「${selectedSlug}」？\n\n此为软删除，索引中会移除。`);
    if (!ok) return;
    setSaving(true);
    try {
      await deleteMemoryEntry(selectedSlug);
      showToast({ message: '条目已删除', type: 'success' });
      setSelectedSlug(null);
      setSummaryDraft('');
      setContentDraft('');
      setEntryDirty(false);
      const data = await fetchMemoryOverview();
      setOverview(data);
    } catch (e) {
      showToast({ message: parseApiError(e), type: 'error' });
    } finally {
      setSaving(false);
    }
  };

  const handleSaveIndex = async () => {
    setSaving(true);
    try {
      await updateMemoryIndex(indexDraft);
      setIndexDirty(false);
      showToast({ message: 'MEMORY.md 已保存', type: 'success' });
      const data = await fetchMemoryOverview();
      setOverview(data);
      setIndexDraft(data.index_raw);
    } catch (e) {
      showToast({ message: parseApiError(e), type: 'error' });
    } finally {
      setSaving(false);
    }
  };

  const handleDreamToggle = async (enabled: boolean) => {
    if (enabled) {
      const confirmed = window.confirm(
        '开启 Dream 会读取近期会话并额外调用一次 LLM 来沉淀长期知识，可能产生额外费用。仅会处理下方指定的项目范围。是否继续？',
      );
      if (!confirmed) return;
      if (!dreamProject.trim()) {
        showToast({ message: '请先填写 Dream 项目标识', type: 'error' });
        return;
      }
    }
    await saveLocalConfig({
      memory: {
        dreamEnabled: enabled,
        dreamProject: dreamProject.trim(),
      },
    });
    showToast({ message: enabled ? 'Dream 已开启' : 'Dream 已关闭', type: 'success' });
  };

  if (loading) {
    return (
      <div className="flex flex-col items-center justify-center py-16 text-gray-400 dark:text-gray-500">
        <Loader2 size={24} className="animate-spin mb-3" />
        <p className="text-sm">加载长期记忆…</p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="space-y-4">
        <div className="rounded-xl border border-red-200 dark:border-red-500/30 p-5 bg-red-50/50 dark:bg-red-500/5">
          <div className="flex items-start gap-3">
            <AlertCircle size={18} className="text-red-500 shrink-0 mt-0.5" />
            <div>
              <p className="text-sm font-medium text-red-700 dark:text-red-300">无法加载长期记忆</p>
              <p className="text-xs text-red-600/80 dark:text-red-400/80 mt-1 leading-relaxed">{error}</p>
              <p className="text-xs text-gray-500 dark:text-gray-400 mt-2">
                请从 <code className="font-mono">http://127.0.0.1:3456</code> 打开应用；若仍失败，再运行 <code className="font-mono">pwcli daemon status</code>。
              </p>
            </div>
          </div>
        </div>
        <button
          onClick={() => void loadOverview()}
          className="flex items-center gap-1.5 px-3 py-2 text-xs font-medium rounded-lg border border-gray-200 dark:border-white/10 hover:bg-gray-100 dark:hover:bg-white/10 text-gray-600 dark:text-gray-300 transition-colors"
        >
          <RefreshCw size={13} />
          重试
        </button>
      </div>
    );
  }

  const stats = overview?.stats;
  const entries = overview?.entries ?? [];
  const entryEditorOpen = isNewEntry || selectedSlug !== null;

  return (
    <div className="space-y-4">
      <div className="rounded-xl border border-gray-200 dark:border-white/10 p-4 bg-white dark:bg-white/[0.02]">
        <div className="flex items-center justify-between gap-3">
          <div>
            <div className="text-sm font-medium text-gray-900 dark:text-white">Dream 沉淀</div>
            <p className="mt-1 text-[11px] text-gray-500 dark:text-gray-400">默认关闭。开启后按项目隔离读取近期会话，并额外调用 LLM 生成可复用记忆。</p>
          </div>
          <button
            type="button"
            onClick={() => void handleDreamToggle(!localConfig.memory?.dreamEnabled)}
            className={`rounded-lg px-3 py-1.5 text-xs font-medium ${localConfig.memory?.dreamEnabled ? 'bg-purple-600 text-white' : 'border border-gray-200 text-gray-600 dark:border-white/10 dark:text-gray-300'}`}
          >
            {localConfig.memory?.dreamEnabled ? '已开启' : '已关闭'}
          </button>
        </div>
        <input
          value={dreamProject}
          onChange={event => setDreamProject(event.target.value)}
          onBlur={() => void saveLocalConfig({ memory: { dreamProject: dreamProject.trim() } })}
          disabled={localConfig.memory?.dreamEnabled}
          placeholder="项目标识，例如 personal-workbench"
          className="input-field mt-3 w-full text-sm font-mono dark:bg-white/5 dark:border-white/10 dark:text-white disabled:opacity-60"
        />
      </div>
      {/* Header + stats */}
      <div className="settings-note">
        <div className="flex items-start justify-between gap-3">
          <div className="flex items-start gap-2">
            <Brain size={16} className="text-gray-500 dark:text-gray-400 shrink-0 mt-0.5" />
            <div>
              <p className="text-xs leading-relaxed">
                跨会话个人记忆，存储于{' '}
                <code>
                  ~/.pwcli/memory/{overview?.user_slug ?? 'local'}/
                </code>
              </p>
              {stats && (
                <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-1">
                  {stats.active_entries} 条活跃 · 索引 {stats.index_bytes} B · 画像{' '}
                  {stats.has_profile ? `${stats.profile_bytes} B` : '未设置'}
                  {stats.vector_indexed != null ? ` · 向量 ${stats.vector_indexed}` : ''}
                </p>
              )}
            </div>
          </div>
          <button
            onClick={() => void loadOverview()}
            disabled={saving}
            className="flex items-center gap-1 px-2 py-1 text-[11px] rounded-md border border-gray-300 dark:border-white/15 hover:bg-gray-100 dark:hover:bg-white/10 text-gray-600 dark:text-gray-300 transition-colors shrink-0"
          >
            <RefreshCw size={11} />
            刷新
          </button>
        </div>
      </div>

      {/* Sub tabs */}
      <div className="flex gap-1 p-1 rounded-lg bg-gray-100 dark:bg-white/[0.04] w-fit">
        {([
          ['profile', '用户画像'],
          ['facts', '核心事实'],
        ] as const).map(([key, label]) => (
          <button
            key={key}
            onClick={() => setSubTab(key)}
            className={`px-3 py-1.5 text-xs font-medium rounded-md transition-colors ${
              subTab === key
                ? 'bg-white dark:bg-white/10 text-purple-600 dark:text-purple-400 shadow-sm'
                : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {subTab === 'profile' && (
        <div className="rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02] space-y-3">
          <div className="flex items-center justify-between">
            <div>
              <h3 className="text-sm font-medium text-gray-900 dark:text-white">PROFILE.md</h3>
              <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-0.5">用户画像摘要，上限 4 KB</p>
            </div>
            <button
              onClick={() => void handleSaveProfile()}
              disabled={saving || !profileDirty}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-lg bg-purple-500 text-white hover:bg-purple-600 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            >
              {saving ? <Loader2 size={13} className="animate-spin" /> : <Save size={13} />}
              保存
            </button>
          </div>
          <textarea
            value={profileDraft}
            onChange={e => {
              setProfileDraft(e.target.value);
              setProfileDirty(true);
            }}
            rows={14}
            placeholder="描述你的身份、偏好、工作习惯…"
            className="input-field w-full text-sm font-mono resize-y dark:bg-white/5 dark:border-white/10 dark:text-white leading-relaxed"
          />
        </div>
      )}

      {subTab === 'facts' && (
        <div className="flex gap-4 min-h-[360px]">
          {/* Left list */}
          <div className="w-[220px] shrink-0 flex flex-col rounded-xl border border-gray-200 dark:border-white/10 bg-white dark:bg-white/[0.02] overflow-hidden">
            <div className="flex items-center justify-between px-3 py-2 border-b border-gray-200 dark:border-white/10">
              <span className="text-xs font-medium text-gray-600 dark:text-gray-300">条目</span>
              <button
                onClick={handleStartNewEntry}
                className="p-1 rounded-md hover:bg-gray-100 dark:hover:bg-white/10 text-purple-600 dark:text-purple-400 transition-colors"
                title="新增条目"
              >
                <Plus size={14} />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto max-h-[400px]">
              {entries.length === 0 && !isNewEntry && (
                <p className="text-xs text-gray-400 dark:text-gray-500 p-3 text-center">暂无条目</p>
              )}
              {entries.map(entry => (
                <button
                  key={entry.slug}
                  onClick={() => handleSelectEntry(entry.slug)}
                  className={`w-full text-left px-3 py-2.5 border-b border-gray-100 dark:border-white/[0.04] transition-colors ${
                    selectedSlug === entry.slug && !isNewEntry
                      ? 'bg-gray-100 dark:bg-white/[0.08]'
                      : 'hover:bg-gray-50 dark:hover:bg-white/[0.03]'
                  }`}
                >
                  <p className="text-xs font-mono font-medium text-gray-800 dark:text-gray-200 truncate">
                    {entry.slug}
                  </p>
                  <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-0.5 line-clamp-2 leading-snug">
                    {entry.summary || '（无摘要）'}
                  </p>
                </button>
              ))}
              {isNewEntry && (
                <div className="px-3 py-2.5 bg-gray-100 dark:bg-white/[0.08] border-b border-gray-200 dark:border-white/10">
                  <p className="text-xs font-mono font-medium text-gray-800 dark:text-gray-200">+ 新条目</p>
                </div>
              )}
            </div>
          </div>

          {/* Right editor */}
          <div className="flex-1 rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02] flex flex-col min-w-0">
            {!entryEditorOpen ? (
              <div className="flex-1 flex items-center justify-center text-gray-400 dark:text-gray-500">
                <p className="text-sm">选择左侧条目，或点击 + 新增</p>
              </div>
            ) : (
              <>
                <div className="flex items-center justify-between mb-3 gap-2">
                  <div className="min-w-0 flex-1">
                    {isNewEntry ? (
                      <input
                        type="text"
                        value={newSlug}
                        onChange={e => {
                          setNewSlug(e.target.value);
                          setEntryDirty(true);
                        }}
                        placeholder="slug（如 coding_style）"
                        className="input-field w-full text-sm font-mono dark:bg-white/5 dark:border-white/10 dark:text-white"
                      />
                    ) : (
                      <h3 className="text-sm font-mono font-medium text-gray-900 dark:text-white truncate">
                        {selectedSlug}
                      </h3>
                    )}
                    {!isNewEntry && selectedSlug && overview?.entries.find(e => e.slug === selectedSlug) && (
                      <p className="text-[11px] text-gray-400 dark:text-gray-500 mt-0.5">
                        更新于 {formatTs(overview.entries.find(e => e.slug === selectedSlug)!.updated_at)}
                      </p>
                    )}
                  </div>
                  <div className="flex items-center gap-1.5 shrink-0">
                    {!isNewEntry && selectedSlug && (
                      <button
                        onClick={() => void handleDeleteEntry()}
                        disabled={saving}
                        className="p-1.5 rounded-lg border border-red-200 dark:border-red-500/30 text-red-500 hover:bg-red-50 dark:hover:bg-red-500/10 disabled:opacity-40 transition-colors"
                        title="删除条目"
                      >
                        <Trash2 size={14} />
                      </button>
                    )}
                    <button
                      onClick={() => void handleSaveEntry()}
                      disabled={saving || entryLoading || (!entryDirty && !isNewEntry)}
                      className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-lg bg-purple-500 text-white hover:bg-purple-600 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                    >
                      {saving || entryLoading ? (
                        <Loader2 size={13} className="animate-spin" />
                      ) : (
                        <Save size={13} />
                      )}
                      {isNewEntry ? '创建' : '保存'}
                    </button>
                  </div>
                </div>

                {entryLoading ? (
                  <div className="flex-1 flex items-center justify-center text-gray-400">
                    <Loader2 size={20} className="animate-spin" />
                  </div>
                ) : (
                  <div className="flex-1 flex flex-col gap-3 min-h-0">
                    <div>
                      <label className="text-[11px] text-gray-500 dark:text-gray-400 mb-1 block">
                        摘要（≤150 字）
                      </label>
                      <input
                        type="text"
                        value={summaryDraft}
                        onChange={e => {
                          setSummaryDraft(e.target.value);
                          setEntryDirty(true);
                        }}
                        maxLength={150}
                        className="input-field w-full text-sm dark:bg-white/5 dark:border-white/10 dark:text-white"
                      />
                      <p className="text-[10px] text-gray-400 dark:text-gray-500 mt-0.5 text-right">
                        {summaryDraft.length}/150
                      </p>
                    </div>
                    <div className="flex-1 flex flex-col min-h-0">
                      <label className="text-[11px] text-gray-500 dark:text-gray-400 mb-1 block">
                        详情（≤16 KB）
                      </label>
                      <textarea
                        value={contentDraft}
                        onChange={e => {
                          setContentDraft(e.target.value);
                          setEntryDirty(true);
                        }}
                        rows={12}
                        className="input-field w-full flex-1 text-sm font-mono resize-y dark:bg-white/5 dark:border-white/10 dark:text-white leading-relaxed min-h-[200px]"
                      />
                    </div>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      )}

      {/* Collapsible MEMORY.md raw */}
      <div className="rounded-xl border border-gray-200 dark:border-white/10 bg-white dark:bg-white/[0.02] overflow-hidden">
        <button
          onClick={() => setShowIndexRaw(v => !v)}
          className="w-full flex items-center gap-2 px-4 py-3 text-left hover:bg-gray-50 dark:hover:bg-white/[0.03] transition-colors"
        >
          {showIndexRaw ? (
            <ChevronDown size={14} className="text-gray-400" />
          ) : (
            <ChevronRight size={14} className="text-gray-400" />
          )}
          <span className="text-xs font-medium text-gray-700 dark:text-gray-300">MEMORY.md 原文</span>
          <span className="text-[10px] text-gray-400 dark:text-gray-500">（高级，上限 8 KB）</span>
        </button>
        {showIndexRaw && (
          <div className="px-4 pb-4 border-t border-gray-100 dark:border-white/[0.06]">
            <div className="flex justify-end mt-3 mb-2">
              <button
                onClick={() => void handleSaveIndex()}
                disabled={saving || !indexDirty}
                className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-lg bg-purple-500 text-white hover:bg-purple-600 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
              >
                {saving ? <Loader2 size={13} className="animate-spin" /> : <Save size={13} />}
                保存索引
              </button>
            </div>
            <textarea
              value={indexDraft}
              onChange={e => {
                setIndexDraft(e.target.value);
                setIndexDirty(true);
              }}
              rows={10}
              className="input-field w-full text-xs font-mono resize-y dark:bg-white/5 dark:border-white/10 dark:text-white leading-relaxed"
            />
          </div>
        )}
      </div>
    </div>
  );
}
