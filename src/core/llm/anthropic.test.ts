import { describe, expect, it } from 'vitest';
import type { ToolSchema } from './types';
import { buildAnthropicTools, normalizeAnthropicInputSchema } from './anthropic';

const schemaWithRootUnion = {
  type: 'object',
  properties: {
    domain: { type: 'string' },
    value: {
      oneOf: [{ type: 'string' }, { type: 'array' }],
    },
  },
  oneOf: [{ required: ['domain'] }, { required: ['domains'] }],
  anyOf: [{ required: ['domain'] }],
  allOf: [{ required: ['domain'] }],
};

describe('normalizeAnthropicInputSchema', () => {
  it('preserves the complete schema by default', () => {
    expect(normalizeAnthropicInputSchema(schemaWithRootUnion, true)).toBe(schemaWithRootUnion);
  });

  it('removes only root combinators for restricted providers', () => {
    const normalized = normalizeAnthropicInputSchema(schemaWithRootUnion, false);

    expect(normalized).not.toHaveProperty('oneOf');
    expect(normalized).not.toHaveProperty('anyOf');
    expect(normalized).not.toHaveProperty('allOf');
    expect(normalized).toHaveProperty('properties.value.oneOf');
    expect(schemaWithRootUnion).toHaveProperty('oneOf');
  });
});

describe('buildAnthropicTools', () => {
  it('applies restricted-schema normalization to every tool', () => {
    const tools = Array.from({ length: 5 }, (_, index) => ({
      type: 'function',
      function: {
        name: `tool_${index}`,
        description: `tool ${index}`,
        parameters: schemaWithRootUnion,
      },
    })) as unknown as ToolSchema[];

    const serialized = buildAnthropicTools(tools, false)!;

    expect(serialized).toHaveLength(5);
    for (const tool of serialized) {
      expect(tool.input_schema).not.toHaveProperty('oneOf');
      expect(tool.input_schema).not.toHaveProperty('anyOf');
      expect(tool.input_schema).not.toHaveProperty('allOf');
    }
  });
});
