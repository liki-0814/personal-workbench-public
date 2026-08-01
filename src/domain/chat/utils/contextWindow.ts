import { getModelMeta } from '@/core/config/modelMetadata';
import { getModelInfo, resolveModelId } from '@/core/config/aiProviders';
import { estimateTokens } from '@/core/llm';
import type { ChatAttachment, ChatMessage } from '../types';

const MESSAGE_OVERHEAD_TOKENS = 4;
const IMAGE_ESTIMATE_TOKENS = 1_024;

export interface ContextWindowUsage {
  usedTokens: number;
  windowTokens: number;
  exactWindow: boolean;
  exactUsage?: boolean;
}

function fallbackContextWindow(model: string): number {
  const id = model.toLowerCase();
  if (id.includes('gpt-5')) return 200_000;
  if (id.includes('gpt-4o') || id.includes('gpt-4-turbo') || id.includes('gpt-4.1')) return 128_000;
  if (id.includes('gpt-4')) return 32_000;
  if (id.includes('gpt-3.5')) return 16_000;
  if (id.includes('opus-4') || id.includes('sonnet-4') || id.includes('haiku-4') || id.includes('claude-3')) return 200_000;
  if (id.includes('claude')) return 100_000;
  if (id.includes('gemini-1.5') || id.includes('gemini-2') || id.includes('gemini-3')) return 1_000_000;
  if (id.includes('qwen') || id.includes('kimi')) return 128_000;
  return 32_000;
}

export function getContextWindow(model: string): Pick<ContextWindowUsage, 'windowTokens' | 'exactWindow'> {
  const resolvedModel = resolveModelId(model);
  const configuredWindow = getModelInfo(model)?.contextWindow;
  if (configuredWindow && configuredWindow > 0) {
    return { windowTokens: configuredWindow, exactWindow: true };
  }
  const metadata = getModelMeta(resolvedModel);
  return metadata
    ? { windowTokens: metadata.contextWindow, exactWindow: true }
    : { windowTokens: fallbackContextWindow(resolvedModel), exactWindow: false };
}

function estimateChatMessageTokens(message: ChatMessage): number {
  const toolCalls = message.tool_calls?.reduce((sum, call) => (
    sum + estimateTokens(call.function.name) + estimateTokens(call.function.arguments) + 4
  ), 0) ?? 0;
  const images = (message.images?.length ?? 0) * IMAGE_ESTIMATE_TOKENS;
  return MESSAGE_OVERHEAD_TOKENS + estimateTokens(message.content) + toolCalls + images;
}

export function estimateVisibleContextTokens(messages: ChatMessage[]): number {
  return messages.reduce((sum, message) => sum + estimateChatMessageTokens(message), 0);
}

export function estimateDraftTokens(
  value: string,
  images: string[] = [],
  attachments: ChatAttachment[] = [],
): number {
  const extraTokens = estimateDraftExtrasTokens(images, attachments);
  return estimateDraftTokensWithExtras(value, extraTokens);
}

export function estimateDraftTokensWithExtras(value: string, extraTokens: number): number {
  if (!value && extraTokens === 0) return 0;
  return MESSAGE_OVERHEAD_TOKENS + estimateTokens(value) + extraTokens;
}

export function estimateDraftExtrasTokens(
  images: string[] = [],
  attachments: ChatAttachment[] = [],
): number {
  const attachmentTokens = attachments.reduce(
    (sum, attachment) => sum + estimateTokens(attachment.content),
    0,
  );
  return images.length * IMAGE_ESTIMATE_TOKENS + attachmentTokens;
}

export function formatTokenAmount(tokens: number): string {
  if (tokens >= 1_000_000) {
    const value = tokens / 1_000_000;
    return `${value >= 10 ? value.toFixed(0) : value.toFixed(1).replace(/\.0$/, '')}M`;
  }
  if (tokens >= 1_000) {
    const value = tokens / 1_000;
    return `${value >= 100 ? value.toFixed(0) : value.toFixed(1).replace(/\.0$/, '')}K`;
  }
  return String(tokens);
}

export function getContextUsagePercent(usedTokens: number, windowTokens: number): number {
  if (windowTokens <= 0) return 0;
  return Math.min(999, usedTokens / windowTokens * 100);
}
