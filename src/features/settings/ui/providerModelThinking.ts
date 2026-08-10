import type { ModelEntry, ThinkingLevel } from '@/core/config';

export function applyThinkingLevelsToModels(
  models: ModelEntry[],
  modelIds: ReadonlySet<string>,
  levels: readonly ThinkingLevel[],
): ModelEntry[] {
  const enabledLevels = levels.filter(level => level !== 'off');
  return models.map(model => {
    if (!modelIds.has(model.id)) return model;
    if (enabledLevels.length === 0) {
      return {
        ...model,
        capabilities: { ...(model.capabilities || {}), thinking: false },
        thinkingLevelMap: undefined,
      };
    }
    return {
      ...model,
      capabilities: { ...(model.capabilities || {}), thinking: true },
      thinkingLevelMap: Object.fromEntries(enabledLevels.map(level => [level, level])),
    };
  });
}

export function applyModelTokenLimits(
  models: ModelEntry[],
  modelIds: ReadonlySet<string>,
  limits: { contextWindow?: number; maxOutput?: number },
): ModelEntry[] {
  return models.map(model => modelIds.has(model.id) ? { ...model, ...limits } : model);
}

export function applyModelVisionCapability(
  models: ModelEntry[],
  modelIds: ReadonlySet<string>,
  supported: boolean,
): ModelEntry[] {
  return models.map(model => modelIds.has(model.id) ? {
    ...model,
    capabilities: { ...(model.capabilities || {}), vision: supported },
  } : model);
}
