import { describe, expect, it } from 'vitest';
import type { ToolSchema } from './types';
import {
  applyAnthropicThinking,
  buildAnthropicTools,
  isRestrictedAnthropicSchemaError,
} from './anthropic';
import { normalizeToolInputSchema } from './toolSchema';

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

describe('normalizeToolInputSchema', () => {
  it('preserves the complete schema by default', () => {
    expect(normalizeToolInputSchema(schemaWithRootUnion, true)).toBe(schemaWithRootUnion);
  });

  it('removes only root combinators for restricted providers', () => {
    const normalized = normalizeToolInputSchema(schemaWithRootUnion, false);

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

describe('isRestrictedAnthropicSchemaError', () => {
  it('matches the provider error independently of the tool index', () => {
    expect(isRestrictedAnthropicSchemaError(
      400,
      'tools.15.custom.input_schema: input_schema does not support oneOf, allOf, or anyOf at the top level',
    )).toBe(true);
    expect(isRestrictedAnthropicSchemaError(500, 'input_schema does not support oneOf')).toBe(false);
    expect(isRestrictedAnthropicSchemaError(400, 'unrelated bad request')).toBe(false);
  });
});

describe('applyAnthropicThinking', () => {
  it('uses adaptive thinking and output_config effort by default', () => {
    const body: Record<string, unknown> = {};
    applyAnthropicThinking(body, {}, 'xhigh');
    expect(body.thinking).toEqual({ type: 'adaptive' });
    expect(body.output_config).toEqual({ effort: 'xhigh' });
  });

  it('keeps explicitly configured legacy manual thinking', () => {
    const body: Record<string, unknown> = {};
    applyAnthropicThinking(body, { budget_tokens: 4096 }, 'minimal');
    expect(body.thinking).toEqual({ type: 'enabled', budget_tokens: 4096 });
    expect(body.output_config).toEqual({ effort: 'low' });
  });

  it('maps ultra to the valid max effort and never emits reasoning_effort', () => {
    const body: Record<string, unknown> = {};
    applyAnthropicThinking(body, { reasoning_effort: 'ultra' }, 'ultra');
    expect(body.output_config).toEqual({ effort: 'max' });
    expect(body).not.toHaveProperty('reasoning_effort');
  });
});
