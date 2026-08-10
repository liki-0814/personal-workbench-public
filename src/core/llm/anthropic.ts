import type { ToolCall, ToolSchema, AgentMessage } from './types';
import { readSSE, type StreamDelta } from './openai';
import { parseDataUrl } from './media';

interface AnthropicContentBlock extends Record<string, unknown> {
  type: string;
}

export async function* streamAnthropic(res: Response): AsyncGenerator<StreamDelta, void, unknown> {
  const contentType = res.headers?.get?.('content-type') || '';
  if (!contentType.includes('text/event-stream')) {
    const data = await res.json();
    const result = parseAnthropicResponse(data);
    yield {
      content: result.content,
      generatedImages: result.generatedImages,
      tool_calls: result.tool_calls,
      finish_reason: result.stop_reason,
    };
    return;
  }

  if (!res.body) throw new Error('No response body');
  const reader = res.body.getReader();
  let text = '';
  let thinkingText = '';
  const toolUseMap = new Map<number, ToolCall>();
  let finishReason: string | undefined;

  for await (const evt of readSSE(reader)) {
    if (evt.done) break;
    const data = evt.data as Record<string, unknown>;
    const type = data?.type as string;

    if (type === 'content_block_delta') {
      const delta = data.delta as Record<string, unknown>;
      const idx = typeof data.index === 'number' ? data.index : 0;
      if (delta?.type === 'thinking_delta' && typeof delta.thinking === 'string') {
        thinkingText += delta.thinking;
      } else if (delta?.type === 'text_delta' && typeof delta.text === 'string') {
        text += delta.text;
      } else if (delta?.type === 'input_json_delta' && typeof delta.partial_json === 'string') {
        const existing = toolUseMap.get(idx) || {
          id: '',
          type: 'function' as const,
          function: { name: '', arguments: '' },
        };
        existing.function.arguments += delta.partial_json;
        toolUseMap.set(idx, existing);
      }
    } else if (type === 'content_block_start') {
      const block = data.content_block as Record<string, unknown>;
      const idx = typeof data.index === 'number' ? data.index : 0;
      if (block?.type === 'tool_use') {
        toolUseMap.set(idx, {
          id: typeof block.id === 'string' ? block.id : '',
          type: 'function',
          function: {
            name: typeof block.name === 'string' ? block.name : '',
            arguments: '',
          },
        });
      }
    } else if (type === 'message_delta') {
      const delta = data.delta as Record<string, unknown>;
      if (typeof delta?.stop_reason === 'string') {
        finishReason = delta.stop_reason;
      }
    }

    yield {
      content: text,
      thinking: thinkingText || undefined,
      tool_calls: toolUseMap.size > 0
        ? Array.from(toolUseMap.entries()).sort((a, b) => a[0] - b[0]).map(([, v]) => v)
        : undefined,
      finish_reason: finishReason,
    };
  }
}

export function parseAnthropicResponse(data: Record<string, unknown>): {
  content: string;
  generatedImages?: string[];
  tool_calls?: ToolCall[];
  stop_reason?: string;
} {
  const contentBlocks = Array.isArray(data.content) ? data.content : [];
  let text = '';
  const generatedImages: string[] = [];
  const tool_calls: ToolCall[] = [];

  for (const block of contentBlocks) {
    if (block && typeof block === 'object') {
      const b = block as Record<string, unknown>;
      if (b.type === 'text' && typeof b.text === 'string') {
        text += b.text;
      } else if (b.type === 'image' && b.source && typeof b.source === 'object') {
        const source = b.source as Record<string, unknown>;
        if (source.type === 'base64' && typeof source.data === 'string' && typeof source.media_type === 'string') {
          generatedImages.push(`data:${source.media_type};base64,${source.data}`);
        }
      } else if (b.type === 'tool_use') {
        tool_calls.push({
          id: typeof b.id === 'string' ? b.id : '',
          type: 'function',
          function: {
            name: typeof b.name === 'string' ? b.name : '',
            arguments: JSON.stringify(b.input && typeof b.input === 'object' ? b.input : {}),
          },
        });
      }
    }
  }

  return {
    content: text,
    generatedImages: generatedImages.length > 0 ? generatedImages : undefined,
    tool_calls: tool_calls.length ? tool_calls : undefined,
    stop_reason: typeof data.stop_reason === 'string' ? data.stop_reason : undefined,
  };
}

export function normalizeAnthropicInputSchema(
  schema: Record<string, unknown>,
  supportsRootCombinators: boolean,
): Record<string, unknown> {
  if (supportsRootCombinators) return schema;
  const normalized = { ...schema };
  delete normalized.oneOf;
  delete normalized.anyOf;
  delete normalized.allOf;
  if (!normalized.type) normalized.type = 'object';
  return normalized;
}

export function isRestrictedAnthropicSchemaError(status: number, body: string): boolean {
  return status === 400
    && body.includes('input_schema')
    && body.includes('does not support')
    && ['oneOf', 'anyOf', 'allOf'].some(keyword => body.includes(keyword));
}

export function buildAnthropicTools(
  tools?: ToolSchema[],
  supportsRootCombinators = true,
) {
  if (!tools) return undefined;
  return tools.map(t => ({
    name: t.function.name,
    description: t.function.description,
    input_schema: normalizeAnthropicInputSchema(
      t.function.parameters as Record<string, unknown>,
      supportsRootCombinators,
    ),
  }));
}

export function toAnthropicMessages(
  msgs: AgentMessage[],
  systemPrompt?: string
): { system: string; messages: { role: 'user' | 'assistant'; content: string | AnthropicContentBlock[] }[] } {
  const systemParts: string[] = [];
  if (systemPrompt) systemParts.push(systemPrompt);
  systemParts.push(...msgs.filter(m => m.role === 'system').map(m => m.content));
  const system = systemParts.join('\n');
  const others = msgs.filter(m => m.role !== 'system');

  const anthropicMsgs: { role: 'user' | 'assistant'; content: string | AnthropicContentBlock[] }[] = [];

  for (const msg of others) {
    if (msg.role === 'user') {
      if (msg.images && msg.images.length > 0) {
        const content: AnthropicContentBlock[] = [{ type: 'text', text: msg.content }];
        for (const img of msg.images) {
          const parsed = parseDataUrl(img);
          if (parsed) {
            content.push({
              type: 'image',
              source: { type: 'base64', media_type: parsed.mediaType, data: parsed.data },
            });
          }
        }
        anthropicMsgs.push({ role: 'user', content });
      } else {
        anthropicMsgs.push({ role: 'user', content: msg.content });
      }
    } else if (msg.role === 'assistant') {
      const content: AnthropicContentBlock[] = [];
      if (msg.content) content.push({ type: 'text', text: msg.content });
      if (msg.tool_calls) {
        for (const tc of msg.tool_calls) {
          content.push({
            type: 'tool_use',
            id: tc.id,
            name: tc.function.name,
            input: JSON.parse(tc.function.arguments || '{}'),
          });
        }
      }
      anthropicMsgs.push({ role: 'assistant', content });
    } else if (msg.role === 'tool') {
      anthropicMsgs.push({
        role: 'user',
        content: [{ type: 'tool_result', tool_use_id: msg.tool_call_id, content: msg.content }],
      });
    }
  }

  return { system, messages: anthropicMsgs };
}
