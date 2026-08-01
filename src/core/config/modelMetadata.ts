import modelMetadataCsv from '../../../pwcli/resources/model_metadata.csv?raw';

/**
 * 模型元数据静态表。唯一真源是 pwcli/resources/model_metadata.csv。
 *
 * 用途：
 * - SettingsModal 的"自动"按钮按 model id 回填 maxOutput
 * - aiProviders.getDefaultMaxOutput 命中时优先用静态表，未命中走 opus→128K / 其他→64K 兜底
 * - Chat 上下文窗口展示与 pwcli 的压缩阈值使用同一组数据
 *
 * 模糊匹配：getModelMeta 会把传入的 id 与表 key 都归一化（小写 + 剥离非字母数字字符），
 * 所以 "Claude-Opus-4-6" / "claude opus 4.6" / "claude_opus_4_6" 都能命中 claude-opus-4-6。
 */
export interface ModelMeta {
  contextWindow: number;
  maxInput: number;
  maxOutput: number;
}

const MODEL_METADATA: Record<string, ModelMeta> = {};
for (const line of modelMetadataCsv.trim().split(/\r?\n/).slice(1)) {
  const [id, contextWindowRaw, maxInputRaw, maxOutputRaw] = line
    .split(',')
    .map(value => value.trim());
  const metadata = {
    contextWindow: Number(contextWindowRaw),
    maxInput: Number(maxInputRaw),
    maxOutput: Number(maxOutputRaw),
  };
  if (
    id
    && Number.isFinite(metadata.contextWindow)
    && Number.isFinite(metadata.maxInput)
    && Number.isFinite(metadata.maxOutput)
  ) {
    MODEL_METADATA[id] = metadata;
  }
}

function normalizeModelKey(id: string): string {
  return id.toLowerCase().replace(/[^a-z0-9]/g, '');
}

const NORMALIZED_INDEX: Record<string, ModelMeta> = (() => {
  const idx: Record<string, ModelMeta> = {};
  for (const [k, v] of Object.entries(MODEL_METADATA)) {
    idx[normalizeModelKey(k)] = v;
  }
  return idx;
})();

export function getModelMeta(id: string): ModelMeta | undefined {
  if (!id) return undefined;
  if (MODEL_METADATA[id]) return MODEL_METADATA[id];
  const normalized = normalizeModelKey(id);
  if (NORMALIZED_INDEX[normalized]) return NORMALIZED_INDEX[normalized];
  // Provider-qualified ids such as `kimi-k3` and `anthropic/claude-opus-4-8`
  // retain the provider prefix. Prefer the longest suffix to avoid a short alias
  // shadowing a more specific model name.
  return Object.entries(NORMALIZED_INDEX)
    .sort(([left], [right]) => right.length - left.length)
    .find(([key]) => normalized.endsWith(key))?.[1];
}
