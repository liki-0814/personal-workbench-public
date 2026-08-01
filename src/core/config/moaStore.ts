import { STORAGE_SYNC_EVENT } from '@/core/storage/syncEngine';
import { DEFAULT_MOA_CONFIG, type MoaConfig } from './moa';
import { getAllModels, type AiModelInfo } from './aiProviders';
import { getLocalConfig, saveLocalConfig } from './localConfig';

function loadStored(): MoaConfig {
  const config = getLocalConfig().ai?.moa;
  return config?.presets ? config : structuredClone(DEFAULT_MOA_CONFIG);
}

let currentConfig = loadStored();
function reload(): void { currentConfig = loadStored(); }
if (typeof window !== 'undefined') window.addEventListener(STORAGE_SYNC_EVENT, reload);

export function getMoaConfig(): MoaConfig { return currentConfig; }
export function setMoaConfig(next: MoaConfig): void { currentConfig = next; }
export async function saveMoaConfig(next: MoaConfig): Promise<void> {
  currentConfig = next;
  await saveLocalConfig({ ai: { moa: next } });
}

export function getProviderModelOptions(): AiModelInfo[] { return getAllModels(); }

const NAME_RE = /^[a-z0-9][a-z0-9._-]{0,63}$/;
export function validateMoaConfig(config: MoaConfig, models = getAllModels()): string[] {
  const errors: string[] = [];
  if (config.activePreset && !config.presets[config.activePreset]) {
    errors.push('当前 preset 不存在');
  }
  const exists = (ref: { provider: string; model: string }) => models.some(model =>
    model.id === ref.model && (!ref.provider || model.providerName === ref.provider));
  for (const [name, preset] of Object.entries(config.presets)) {
    if (!NAME_RE.test(name)) errors.push(`Preset 名称无效: ${name}`);
    if (!exists(preset.judge)) errors.push(`${name}: Judge 模型无效`);
    if (preset.enabled && preset.advisors.length === 0) errors.push(`${name}: 至少需要一个 Advisor 模型`);
    for (const advisor of preset.advisors) {
      if (!exists(advisor)) errors.push(`${name}: Advisor 模型无效`);
    }
    for (const temperature of [preset.advisorTemperature, preset.judgeTemperature]) {
      if (temperature != null && (temperature < 0 || temperature > 2)) {
        errors.push(`${name}: temperature 必须在 0–2`);
      }
    }
    for (const count of [preset.lowRiskAdvisors, preset.elevatedRiskAdvisors, preset.highRiskAdvisors]) {
      if (count != null && (!Number.isInteger(count) || count < 1 || count > preset.advisors.length)) {
        errors.push(`${name}: 风险分层 Advisor 数量必须在 1–${preset.advisors.length} 之间`);
      }
    }
  }
  return errors;
}
