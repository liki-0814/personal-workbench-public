import {
  getModelInfo,
  buildAiRequest,
} from '@/core/config';
import { getModelMeta } from '@/core/config/modelMetadata';
import type { ToolCall, LlmRequest, LlmResponse, LlmStreamEvent } from './types';
import { streamOpenAI, toOpenAIMessages, parseOpenAIMultimodalContent, type StreamDelta } from './openai';
import { streamAnthropic, toAnthropicMessages, buildAnthropicTools, parseAnthropicResponse } from './anthropic';
export { parseOpenAIMultimodalContent } from './openai';
export { parseAnthropicResponse } from './anthropic';
export { parseDataUrl } from './media';

export function getDefaultMaxOutput(modelId: string): number {
  const meta = getModelMeta(modelId);
  if (meta?.maxOutput) return meta.maxOutput;
  return modelId.toLowerCase().includes('opus') ? 128_000 : 64_000;
}

export function estimateTokens(text: string): number {
  if (!text) return 0;
  let count = 0;
  for (const ch of text) {
    count += /[一-龥]/.test(ch) ? 1 : 0.25;
  }
  return Math.max(1, Math.ceil(count));
}

const RESERVED_REQUEST_PARAMS = new Set([
  'model',
  'messages',
  'stream',
  'tools',
  'tool_choice',
  'max_tokens',
  'temperature',
]);

function mergeModelRequestParams(
  body: Record<string, unknown>,
  params?: Record<string, unknown>,
): void {
  if (!params) return;
  for (const [key, value] of Object.entries(params)) {
    if (!RESERVED_REQUEST_PARAMS.has(key)) body[key] = value;
  }
}

// ---------- Unified Streaming API ----------

export async function* streamLlm(
  request: LlmRequest & { systemPrompt?: string },
  signal?: AbortSignal
): AsyncGenerator<LlmStreamEvent, void, unknown> {
  const { model, messages, tools, temperature = 0.7, max_tokens, systemPrompt, stream = true, thinking } = request;
  const info = getModelInfo(model);

  if (!info.baseUrl || !info.apiKey) {
    yield { type: 'error', message: 'AI 模型未配置，请先在设置中添加 AI Provider' };
    return;
  }

  const baseMaxTokens = max_tokens ?? info.maxOutput ?? getDefaultMaxOutput(info.id);
  const effectiveMaxTokens = thinking && baseMaxTokens < 2048 ? 2048 : baseMaxTokens;
  const { url, headers } = buildAiRequest(info);

  let res: Response;
  try {
    if (info.provider === 'anthropic') {
      const { system, messages: anthropicMessages } = toAnthropicMessages(messages, systemPrompt);
      const body: Record<string, unknown> = {
        model: info.id,
        max_tokens: effectiveMaxTokens,
        system,
        messages: anthropicMessages,
        stream,
      };
      if (tools?.length) body.tools = buildAnthropicTools(tools);
      mergeModelRequestParams(body, info.requestParams);
      if (thinking) {
        if (info.thinkingParams && Object.keys(info.thinkingParams).length > 0) {
          mergeModelRequestParams(body, info.thinkingParams);
        } else {
          body.thinking = { type: 'enabled', budget_tokens: 1024 };
        }
      }
      res = await fetch(url, { method: 'POST', signal, headers, body: JSON.stringify(body) });
    } else {
      const body: Record<string, unknown> = {
        model: info.id,
        messages: toOpenAIMessages(messages, systemPrompt),
        temperature,
        stream,
      };
      if (tools?.length) body.tools = tools;
      if (effectiveMaxTokens) body.max_tokens = effectiveMaxTokens;
      mergeModelRequestParams(body, info.requestParams);
      if (thinking) {
        if (info.thinkingParams && Object.keys(info.thinkingParams).length > 0) {
          mergeModelRequestParams(body, info.thinkingParams);
        } else {
          body.enable_thinking = true;
        }
      }
      res = await fetch(url, { method: 'POST', signal, headers, body: JSON.stringify(body) });
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    yield { type: 'error', message: `网络请求失败: ${message}` };
    return;
  }

  if (!res.ok) {
    const text = await res.text().catch(() => '');
    yield { type: 'error', message: `请求失败 (${res.status}): ${text || res.statusText}` };
    return;
  }

  const provider = info.provider;

  if (!stream) {
    let data: Record<string, unknown>;
    try {
      data = await res.json();
    } catch {
      const text = await res.text().catch(() => '');
      yield { type: 'error', message: `无法解析响应: ${text.slice(0, 200) || res.statusText}` };
      return;
    }

    const bodyError = data.error as Record<string, unknown> | undefined;
    if (bodyError) {
      const errMsg = typeof bodyError.message === 'string' ? bodyError.message : JSON.stringify(bodyError);
      yield { type: 'error', message: `API 错误: ${errMsg}` };
      return;
    }

    if (provider === 'anthropic') {
      const parsed = parseAnthropicResponse(data);
      yield { type: 'delta', content: parsed.content, generatedImages: parsed.generatedImages, tool_calls: parsed.tool_calls };
      yield { type: 'done', stop_reason: parsed.stop_reason };
    } else {
      const choice = (data.choices as Array<Record<string, unknown>> | undefined)?.[0];
      const msg = choice?.message as Record<string, unknown> | undefined;
      let content = msg?.content;
      if ((content === '' || content == null) && msg?.reasoning_content) {
        content = msg.reasoning_content;
      }
      const parsed = parseOpenAIMultimodalContent(content);
      yield { type: 'delta', content: parsed.text, generatedImages: parsed.generatedImages, tool_calls: msg?.tool_calls as ToolCall[] | undefined };
      yield { type: 'done', stop_reason: choice?.finish_reason as string | undefined };
    }
    return;
  }

  try {
    const gen = provider === 'anthropic' ? streamAnthropic(res) : streamOpenAI(res);
    let lastDelta: StreamDelta | undefined;
    for await (const delta of gen) {
      lastDelta = delta;
      yield { type: 'delta', content: delta.content, thinking: delta.thinking, generatedImages: delta.generatedImages, tool_calls: delta.tool_calls };
    }
    yield { type: 'done', stop_reason: lastDelta?.finish_reason };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    yield { type: 'error', message: `流式解析失败: ${message}` };
  }
}

// ---------- Unified Non-Streaming API ----------

export async function callLlm(
  request: LlmRequest & { systemPrompt?: string },
  signal?: AbortSignal
): Promise<LlmResponse> {
  const events: LlmStreamEvent[] = [];
  for await (const event of streamLlm({ ...request, stream: false }, signal)) {
    events.push(event);
  }

  const delta = events.find(e => e.type === 'delta') as Extract<LlmStreamEvent, { type: 'delta' }> | undefined;
  const done = events.find(e => e.type === 'done') as Extract<LlmStreamEvent, { type: 'done' }> | undefined;
  const error = events.find(e => e.type === 'error') as Extract<LlmStreamEvent, { type: 'error' }> | undefined;

  if (error) {
    throw new Error(error.message);
  }

  return {
    content: delta?.content ?? '',
    tool_calls: delta?.tool_calls,
    stop_reason: done?.stop_reason,
    generatedImages: delta?.generatedImages,
  };
}
