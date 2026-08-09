import type { ToolCall } from './types';

export function parseOpenAIMultimodalContent(content: unknown): { text: string; generatedImages: string[] } {
  const generatedImages: string[] = [];
  let text = '';
  if (Array.isArray(content)) {
    for (const block of content) {
      if (block?.type === 'text' && typeof block.text === 'string') {
        text += block.text;
      } else if (block?.type === 'image_url' && block.image_url?.url) {
        const url = block.image_url.url as string;
        if (url.startsWith('data:')) {
          generatedImages.push(url);
        } else {
          generatedImages.push(`data:image/png;base64,${url}`);
        }
      }
    }
  } else if (typeof content === 'string') {
    text = content;
  }
  return { text, generatedImages };
}

export interface StreamDelta {
  content: string;
  thinking?: string;
  generatedImages?: string[];
  tool_calls?: ToolCall[];
  finish_reason?: string;
}

interface SSEEvent {
  event?: string;
  data: Record<string, unknown> | null;
  done: boolean;
}

export async function* readSSE(reader: ReadableStreamDefaultReader<Uint8Array>): AsyncGenerator<SSEEvent, void, unknown> {
  const decoder = new TextDecoder();
  let buffer = '';
  let currentEvent: string | undefined;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split('\n');
    buffer = lines.pop() || '';
    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) {
        currentEvent = undefined;
        continue;
      }
      if (trimmed.startsWith('event: ')) {
        currentEvent = trimmed.slice(7).trim();
      } else if (trimmed.startsWith('data: ')) {
        const payload = trimmed.slice(6);
        if (payload === '[DONE]') {
          yield { event: currentEvent, data: null, done: true };
          return;
        }
        try {
          yield { event: currentEvent, data: JSON.parse(payload), done: false };
        } catch {
          // ignore malformed JSON
        }
        currentEvent = undefined;
      }
    }
  }
}

export async function* streamOpenAI(res: Response): AsyncGenerator<StreamDelta, void, unknown> {
  const contentType = res.headers?.get?.('content-type') || '';
  if (!contentType.includes('text/event-stream')) {
    const data = await res.json();
    const choice = data.choices?.[0];
    const content = choice?.message?.content;
    const parsed = parseOpenAIMultimodalContent(content);
    yield {
      content: parsed.text,
      generatedImages: parsed.generatedImages.length > 0 ? parsed.generatedImages : undefined,
      tool_calls: choice?.message?.tool_calls as ToolCall[] | undefined,
      finish_reason: choice?.finish_reason as string | undefined,
    };
    return;
  }

  if (!res.body) throw new Error('No response body');
  const reader = res.body.getReader();
  let text = '';
  let thinkingText = '';
  let generatedImages: string[] | undefined;
  const toolCallMap = new Map<number, ToolCall>();
  let finishReason: string | undefined;

  for await (const evt of readSSE(reader)) {
    if (evt.done) break;
    if (evt.event === 'error') {
      const d = evt.data as Record<string, unknown> | null;
      const msg = (d?.message as string) || (d?.code as string) || JSON.stringify(d);
      throw new Error(msg);
    }
    const choice = (evt.data as Record<string, unknown>)?.choices as Array<Record<string, unknown>> | undefined;
    const delta = choice?.[0]?.delta as Record<string, unknown> | undefined;
    const fr = choice?.[0]?.finish_reason as string | undefined;
    if (fr !== undefined && fr !== null) finishReason = fr;

    if (typeof delta?.reasoning_content === 'string') {
      thinkingText += delta.reasoning_content;
    }
    if (typeof delta?.content === 'string') {
      text += delta.content;
    } else if (Array.isArray(delta?.content)) {
      const parsed = parseOpenAIMultimodalContent(delta.content);
      text = parsed.text;
      generatedImages = parsed.generatedImages.length > 0 ? parsed.generatedImages : undefined;
    }
    if (Array.isArray(delta?.tool_calls)) {
      for (const tc of delta.tool_calls as Array<Record<string, unknown>>) {
        const idx = typeof tc.index === 'number' ? tc.index : 0;
        const existing = toolCallMap.get(idx) || {
          id: '',
          type: 'function' as const,
          function: { name: '', arguments: '' },
        };
        if (typeof tc.id === 'string') existing.id = tc.id;
        if (typeof tc.type === 'string') existing.type = tc.type as 'function';
        const fn = tc.function as Record<string, unknown> | undefined;
        if (typeof fn?.name === 'string') existing.function.name = fn.name;
        if (typeof fn?.arguments === 'string') existing.function.arguments += fn.arguments;
        toolCallMap.set(idx, existing);
      }
    }

    yield {
      content: text,
      thinking: thinkingText || undefined,
      generatedImages,
      tool_calls: toolCallMap.size > 0
        ? Array.from(toolCallMap.entries()).sort((a, b) => a[0] - b[0]).map(([, v]) => v)
        : undefined,
      finish_reason: finishReason,
    };
  }
}

export function toOpenAIMessages(msgs: { role: string; content: string; images?: string[]; tool_calls?: ToolCall[]; tool_call_id?: string }[], systemPrompt?: string): unknown[] {
  const result: unknown[] = systemPrompt ? [{ role: 'system', content: systemPrompt }] : [];
  for (const msg of msgs) {
    if (msg.role === 'system') { result.push(msg); continue; }
    if (msg.role === 'user') {
      if (msg.images && msg.images.length > 0) {
        const content: { type: string; text?: string; image_url?: { url: string } }[] = [
          { type: 'text', text: msg.content },
        ];
        for (const img of msg.images) {
          content.push({ type: 'image_url', image_url: { url: img } });
        }
        result.push({ role: 'user', content });
      } else {
        result.push(msg);
      }
    } else if (msg.role === 'assistant') {
      const assistantMsg: Record<string, unknown> = { role: 'assistant', content: msg.content };
      if (msg.tool_calls?.length) assistantMsg.tool_calls = msg.tool_calls;
      result.push(assistantMsg);
    } else if (msg.role === 'tool') {
      result.push({ role: 'tool', tool_call_id: msg.tool_call_id, content: msg.content });
    }
  }
  return result;
}
