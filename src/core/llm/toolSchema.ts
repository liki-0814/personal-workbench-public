import type { ToolSchema } from './types';

export function normalizeToolInputSchema(
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

export function normalizeToolSchemas(
  tools: ToolSchema[],
  supportsRootCombinators: boolean,
): ToolSchema[] {
  if (supportsRootCombinators) return tools;
  return tools.map(tool => ({
    ...tool,
    function: {
      ...tool.function,
      parameters: normalizeToolInputSchema(
        tool.function.parameters as Record<string, unknown>,
        false,
      ) as ToolSchema['function']['parameters'],
    },
  }));
}
