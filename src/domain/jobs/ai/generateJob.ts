import { callLlm } from '@/core/llm';
import { getModels, getFeatureModel } from '@/core/config';

export interface GeneratedJobSpec {
  name: string;
  type: 'command' | 'agent';
  cron: string;
  command?: string;
  prompt?: string;
  cwd?: string;
  testCommand?: string;
}

const NAME_PATTERN = /^[A-Za-z0-9_.-]+$/;

function isGeneratedJobSpec(value: unknown): value is GeneratedJobSpec {
  if (typeof value !== 'object' || value === null) return false;
  const spec = value as Record<string, unknown>;
  if (typeof spec.name !== 'string' || !NAME_PATTERN.test(spec.name)) return false;
  if (spec.type !== 'command' && spec.type !== 'agent') return false;
  if (typeof spec.cron !== 'string' || spec.cron.trim().split(/\s+/).length !== 5) return false;
  if (spec.type === 'command' && (typeof spec.command !== 'string' || !spec.command.trim())) return false;
  if (spec.type === 'agent' && (typeof spec.prompt !== 'string' || !spec.prompt.trim())) return false;
  return true;
}

/** Extract the first balanced JSON object from LLM output text. */
function extractJsonObject(text: string): unknown | null {
  const start = text.indexOf('{');
  if (start < 0) return null;
  let depth = 0;
  for (let i = start; i < text.length; i++) {
    if (text[i] === '{') depth++;
    else if (text[i] === '}') {
      depth--;
      if (depth === 0) {
        try {
          return JSON.parse(text.slice(start, i + 1));
        } catch {
          return null;
        }
      }
    }
  }
  return null;
}

export async function generateJobWithAI(description: string): Promise<GeneratedJobSpec> {
  const systemPrompt = `你是一个定时任务配置助手。用户会描述想自动执行的工作，你需要生成一个调度任务配置。

返回 JSON 对象（不要其他文字），字段：
- name: 任务名，仅字母数字和 _-.，如 "daily-report"
- type: "command"（执行 shell 命令）或 "agent"（调用 AI 执行，适合需要理解/写作的任务）
- cron: 5 段 cron 表达式（分 时 日 月 周），如 "0 8 * * *" 表示每天 8 点
- command: type=command 时必填，要执行的 shell 命令
- prompt: type=agent 时必填，给 AI 的任务指令
- cwd: 工作目录（可选，用户提到目录时填）
- testCommand: type=command 时可选。如果 command 有副作用（写文件、发请求等），提供一个无副作用的替代测试命令（如 --dry-run、echo 验证环境）；无副作用则可省略

示例：
用户"每天早上8点把 ~/notes 下昨天的日记汇总成摘要"
{"name":"daily-note-summary","type":"agent","cron":"0 8 * * *","prompt":"汇总 ~/notes 下昨天的日记，生成摘要","cwd":"~/notes"}

只返回 JSON 对象。`;

  const model = getFeatureModel('task') || getModels()[0]?.id;
  if (!model) throw new Error('未配置 AI 模型');

  const response = await callLlm({
    model,
    messages: [{ role: 'user', content: `我想自动执行的工作：${description}` }],
    systemPrompt,
    temperature: 0.3,
    feature: 'job-generate',
  });

  const parsed = extractJsonObject(response.content || '');
  if (!isGeneratedJobSpec(parsed)) {
    throw new Error('AI 未能生成有效的任务配置，请更具体地描述执行内容和时间');
  }
  return parsed;
}
