import { describe, it, expect } from 'vitest';
import {
  estimateTokens,
  parseDataUrl,
  parseOpenAIMultimodalContent,
  parseAnthropicResponse,
} from '@/core/llm';

describe('llmClient', () => {
  describe('estimateTokens', () => {
    it('should estimate English text tokens', () => {
      expect(estimateTokens('hello world')).toBe(3); // 11 chars * 0.25 = 2.75 -> 3
    });

    it('should estimate Chinese text tokens', () => {
      expect(estimateTokens('你好世界')).toBe(4); // 4 CJK chars = 4 tokens
    });

    it('should handle empty string', () => {
      expect(estimateTokens('')).toBe(0);
    });

    it('should handle mixed text', () => {
      const text = 'hello 世界';
      // 'hello ' = 6 * 0.25 = 1.5; '世界' = 2
      // total = 3.5 -> 4
      expect(estimateTokens(text)).toBe(4);
    });
  });

  describe('parseDataUrl', () => {
    it('should parse valid data URL', () => {
      const result = parseDataUrl('data:image/png;base64,abc123');
      expect(result).toEqual({ mediaType: 'image/png', data: 'abc123' });
    });

    it('should return null for invalid data URL', () => {
      expect(parseDataUrl('not-a-data-url')).toBeNull();
    });
  });

  describe('parseOpenAIMultimodalContent', () => {
    it('should parse text blocks', () => {
      const result = parseOpenAIMultimodalContent([
        { type: 'text', text: 'hello' },
        { type: 'text', text: ' world' },
      ]);
      expect(result.text).toBe('hello world');
      expect(result.generatedImages).toEqual([]);
    });

    it('should parse image blocks', () => {
      const result = parseOpenAIMultimodalContent([
        { type: 'text', text: 'image:' },
        { type: 'image_url', image_url: { url: 'data:image/png;base64,abc' } },
      ]);
      expect(result.generatedImages).toEqual(['data:image/png;base64,abc']);
    });

    it('should convert raw base64 to data URL', () => {
      const result = parseOpenAIMultimodalContent([
        { type: 'image_url', image_url: { url: 'rawbase64data' } },
      ]);
      expect(result.generatedImages[0]).toBe('data:image/png;base64,rawbase64data');
    });

    it('should handle plain string', () => {
      const result = parseOpenAIMultimodalContent('plain text');
      expect(result.text).toBe('plain text');
    });
  });

  describe('parseAnthropicResponse', () => {
    it('should parse text content', () => {
      const result = parseAnthropicResponse({
        content: [{ type: 'text', text: 'hello' }],
        stop_reason: 'end_turn',
      });
      expect(result.content).toBe('hello');
      expect(result.stop_reason).toBe('end_turn');
    });

    it('should parse tool_use', () => {
      const result = parseAnthropicResponse({
        content: [
          { type: 'tool_use', id: 'tu-1', name: 'test_tool', input: { key: 'value' } },
        ],
      });
      expect(result.tool_calls).toHaveLength(1);
      expect(result.tool_calls![0].id).toBe('tu-1');
      expect(result.tool_calls![0].function.name).toBe('test_tool');
      expect(result.tool_calls![0].function.arguments).toBe('{"key":"value"}');
    });

    it('should parse image content', () => {
      const result = parseAnthropicResponse({
        content: [
          {
            type: 'image',
            source: { type: 'base64', media_type: 'image/png', data: 'abc123' },
          },
        ],
      });
      expect(result.generatedImages).toEqual(['data:image/png;base64,abc123']);
    });

    it('should handle empty content', () => {
      const result = parseAnthropicResponse({});
      expect(result.content).toBe('');
    });
  });
});
