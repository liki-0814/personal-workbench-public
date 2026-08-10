import { describe, expect, it } from 'vitest';
import type { ToolSchema } from './types';
import { normalizeToolSchemas } from './toolSchema';

describe('normalizeToolSchemas', () => {
  it('applies the restricted dialect to every OpenAI-style tool', () => {
    const tools = Array.from({ length: 20 }, (_, index) => ({
      type: 'function',
      function: {
        name: `tool_${index}`,
        description: `tool ${index}`,
        parameters: {
          type: 'object',
          properties: { value: { type: 'string' } },
          oneOf: [{ required: ['value'] }],
          anyOf: [{ required: ['value'] }],
          allOf: [{ required: ['value'] }],
        },
      },
    })) as unknown as ToolSchema[];

    const normalized = normalizeToolSchemas(tools, false);
    for (const tool of normalized) {
      expect(tool.function.parameters).not.toHaveProperty('oneOf');
      expect(tool.function.parameters).not.toHaveProperty('anyOf');
      expect(tool.function.parameters).not.toHaveProperty('allOf');
    }
    expect(tools[0].function.parameters).toHaveProperty('oneOf');
  });
});
