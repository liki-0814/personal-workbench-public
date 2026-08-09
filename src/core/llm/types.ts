export interface ToolParameter {
  type: string;
  description?: string;
  enum?: string[];
  properties?: Record<string, ToolParameter>;
  items?: ToolParameter;
  required?: string[];
}

export interface ToolSchema {
  type: 'function';
  function: {
    name: string;
    description: string;
    parameters: {
      type: 'object';
      properties: Record<string, ToolParameter>;
      required?: string[];
    };
  };
}

export type ToolHandler = (args: Record<string, unknown>) => Promise<string> | string;

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: {
    type: 'object';
    properties: Record<string, ToolParameter>;
    required?: string[];
  };
  handler: ToolHandler;
}

export interface ToolCall {
  id: string;
  type: 'function';
  function: {
    name: string;
    arguments: string;
  };
}

export interface ToolResult {
  toolCallId: string;
  name: string;
  output: string;
  error?: boolean;
}

export type AgentMessageRole = 'system' | 'user' | 'assistant' | 'tool';

export interface AgentMessage {
  id?: string;
  role: AgentMessageRole;
  content: string;
  images?: string[];
  tool_calls?: ToolCall[];
  tool_call_id?: string;
}

export interface LlmRequest {
  model: string;
  messages: AgentMessage[];
  tools?: ToolSchema[];
  temperature?: number;
  max_tokens?: number;
  stream?: boolean;
  feature?: string;
  thinking?: boolean;
  thinkingLevel?: import('@/core/config').ThinkingLevel;
}

export interface LlmResponse {
  content: string;
  tool_calls?: ToolCall[];
  stop_reason?: string;
  generatedImages?: string[];
}

export type LlmStreamEvent =
  | { type: 'delta'; content: string; thinking?: string; generatedImages?: string[]; tool_calls?: ToolCall[] }
  | { type: 'done'; stop_reason?: string }
  | { type: 'error'; message: string };
