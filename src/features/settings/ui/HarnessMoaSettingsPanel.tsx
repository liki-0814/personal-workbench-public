import { Sparkles, Trash2 } from 'lucide-react';
import { type MoaConfig, type AiModelInfo, type MoaModelRef, type MoaPreset } from '@/core/config';
import { SelectField } from '@/shell';

interface Props {
  config: MoaConfig;
  modelOptions: AiModelInfo[];
  onChange: (next: MoaConfig) => void;
  onSave: () => void | Promise<void>;
  isDirty: boolean;
  saving?: boolean;
  errors: string[];
}

const EMPTY_REF: MoaModelRef = { provider: '', model: '' };
const refValue = (ref?: MoaModelRef) => ref ? `${ref.provider}\u0000${ref.model}` : '';
const parseRef = (value: string): MoaModelRef => {
  const [provider = '', model = ''] = value.split('\u0000');
  return { provider, model };
};

function ModelSelect({ value, onChange, options, placeholder }: {
  value?: MoaModelRef;
  onChange: (value: MoaModelRef) => void;
  options: AiModelInfo[];
  placeholder: string;
}) {
  const providers = [...new Set(options.map(model => model.providerName ?? 'Provider'))];
  return (
    <SelectField
      value={refValue(value)}
      onValueChange={next => onChange(parseRef(next))}
      options={[
        { value: '', label: placeholder },
        ...providers.flatMap(provider => options
          .filter(model => (model.providerName ?? 'Provider') === provider)
          .map(model => ({
            value: `${provider}\u0000${model.id}`,
            label: model.name,
            group: provider,
          }))),
      ]}
      className="text-xs w-full"
      ariaLabel={placeholder}
    />
  );
}

export default function HarnessMoaSettingsPanel({ config, modelOptions, onChange, onSave, isDirty, saving = false, errors }: Props) {
  const names = Object.keys(config.presets);
  const activeName = config.presets[config.activePreset] ? config.activePreset : names[0] ?? '';
  const preset = config.presets[activeName];
  const visionModels = modelOptions.filter(model => model.capabilities?.vision);
  const updatePreset = (patch: Partial<MoaPreset>) => {
    if (!preset) return;
    onChange({ ...config, presets: { ...config.presets, [activeName]: { ...preset, ...patch } } });
  };

  return (
    <div className="mt-6 space-y-4 rounded-xl border border-gray-200 dark:border-white/10 p-5 bg-white dark:bg-white/[0.02]">
      <div className="flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-2">
            <Sparkles size={16} className="text-purple-500" />
            <h3 className="text-sm font-medium text-gray-900 dark:text-white">关键决策复核</h3>
            {activeName && <span className="settings-meta-badge">{activeName}</span>}
          </div>
          <p className="text-xs text-gray-500 dark:text-gray-400 mt-1">主 Agent 保持当前聊天模型；仅在 Harness 的关键争议节点由 Advisors 独立审查，再交给 Judge 裁决。</p>
        </div>
        {preset && (
          <label className="flex items-center gap-2 text-xs text-gray-600 dark:text-gray-300 shrink-0">
            <input type="checkbox" checked={preset.enabled} onChange={event => updatePreset({ enabled: event.target.checked })} />
            启用
          </label>
        )}
      </div>

      {preset && <>
        <div><label className="text-xs font-medium text-gray-700 dark:text-gray-300 mb-2 block">Judge model</label><ModelSelect value={preset.judge} onChange={judge => updatePreset({ judge })} options={modelOptions} placeholder="选择 Judge" /></div>

        <div>
          <div className="flex justify-between mb-2"><label className="text-xs font-medium text-gray-700 dark:text-gray-300">Advisor models</label><button type="button" onClick={() => updatePreset({ advisors: [...preset.advisors, EMPTY_REF] })} className="settings-text-action text-xs">+ 添加 Advisor</button></div>
          <div className="space-y-2">{preset.advisors.map((advisor, index) => <div key={`${index}:${refValue(advisor)}`} className="flex gap-2"><ModelSelect value={advisor} onChange={value => updatePreset({ advisors: preset.advisors.map((item, itemIndex) => itemIndex === index ? value : item) })} options={modelOptions} placeholder="选择 Advisor" /><button type="button" onClick={() => updatePreset({ advisors: preset.advisors.filter((_, itemIndex) => itemIndex !== index) })} className="p-2 text-red-500"><Trash2 size={14} /></button></div>)}</div>
        </div>

        <details className="rounded-lg border border-gray-200 dark:border-white/10 p-3">
          <summary className="text-xs cursor-pointer text-gray-600 dark:text-gray-300">高级设置</summary>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mt-4">
            <div><label className="text-[11px] text-gray-500 mb-1 block">图片描述模型（可选）</label><ModelSelect value={preset.imageDescriber} onChange={imageDescriber => updatePreset({ imageDescriber })} options={visionModels} placeholder="自动选择" /></div>
            <div><label className="text-[11px] text-gray-500 mb-1 block">Advisor 最大输出 tokens（留空=模型上限）</label><input type="number" min={0} value={preset.advisorMaxTokens ?? ''} onChange={event => updatePreset({ advisorMaxTokens: event.target.value ? Number(event.target.value) : null })} className="input-field text-sm w-full" /></div>
            <div><label className="text-[11px] text-gray-500 mb-1 block">Advisor temperature（留空=Provider 默认）</label><input type="number" min={0} max={2} step={0.1} value={preset.advisorTemperature ?? ''} onChange={event => updatePreset({ advisorTemperature: event.target.value ? Number(event.target.value) : null })} className="input-field text-sm w-full" /></div>
            <div><label className="text-[11px] text-gray-500 mb-1 block">Judge temperature（留空=Provider 默认）</label><input type="number" min={0} max={2} step={0.1} value={preset.judgeTemperature ?? ''} onChange={event => updatePreset({ judgeTemperature: event.target.value ? Number(event.target.value) : null })} className="input-field text-sm w-full" /></div>
            <div><label className="text-[11px] text-gray-500 mb-1 block">低风险 Advisor 数（默认 1）</label><input type="number" min={1} max={preset.advisors.length || 1} value={preset.lowRiskAdvisors ?? ''} onChange={event => updatePreset({ lowRiskAdvisors: event.target.value ? Number(event.target.value) : null })} className="input-field text-sm w-full" /></div>
            <div><label className="text-[11px] text-gray-500 mb-1 block">中风险 Advisor 数（默认 2）</label><input type="number" min={1} max={preset.advisors.length || 1} value={preset.elevatedRiskAdvisors ?? ''} onChange={event => updatePreset({ elevatedRiskAdvisors: event.target.value ? Number(event.target.value) : null })} className="input-field text-sm w-full" /></div>
            <div><label className="text-[11px] text-gray-500 mb-1 block">高风险 Advisor 数（留空=全部）</label><input type="number" min={1} max={preset.advisors.length || 1} value={preset.highRiskAdvisors ?? ''} onChange={event => updatePreset({ highRiskAdvisors: event.target.value ? Number(event.target.value) : null })} className="input-field text-sm w-full" /></div>
          </div>
        </details>
      </>}

      {!preset && <div className="settings-note">当前没有可编辑的固定复核配置，请通过首次配置向导恢复默认的 fusion preset。</div>}

      {errors.length > 0 && <div className="text-xs text-amber-700 bg-amber-50 dark:bg-amber-500/10 rounded-lg px-3 py-2">{errors.map(error => <div key={error}>{error}</div>)}</div>}
      <div className="flex justify-end"><button type="button" onClick={onSave} disabled={!isDirty || saving || errors.length > 0} className="settings-primary-action px-4 py-2 text-xs rounded-lg disabled:opacity-40">{saving ? '保存中…' : '保存复核配置'}</button></div>
    </div>
  );
}
