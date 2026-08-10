import { callLlm } from '@/core/llm';
import { getModels, getFeatureModel, modelSelectionKey } from '@/core/config';

export interface WorkbenchTaskDraft {
  intent: 'task';
  title: string;
  notes: string;
  priority: 'high' | 'medium' | 'low';
  dueDate?: string;
  objectiveTitle?: string;
  keyResultTitle?: string;
  subTasks: { title: string }[];
}

export interface WorkbenchObjectiveDraft {
  intent: 'objective';
  title: string;
  description?: string;
  keyResults: { title: string }[];
}

export interface WorkbenchKeyResultDraft {
  intent: 'key_result';
  objectiveTitle?: string;
  keyResults: { title: string }[];
}

export type WorkbenchAiDraft = WorkbenchTaskDraft | WorkbenchObjectiveDraft | WorkbenchKeyResultDraft;

const SYSTEM_PROMPT = `你是个人工作台的智能收件助手。先识别用户想创建的是任务、目标（O），还是某个目标下的关键结果（KR），再返回对应 JSON。

任务：
{"intent":"task","title":"简短任务标题","notes":"背景、目标或产出","priority":"high|medium|low","dueDate":"YYYY-MM-DD（可选）","objectiveTitle":"匹配到的已有目标名称（可选）","keyResultTitle":"匹配到的已有 KR 编号或名称（可选）","subTasks":[{"title":"具体步骤"}]}

目标：
{"intent":"objective","title":"目标标题","description":"目标边界或成功标准","keyResults":[{"title":"可验证的关键结果"}]}

为已有目标补充 KR：
{"intent":"key_result","objectiveTitle":"目标名称（可选）","keyResults":[{"title":"可验证的关键结果"}]}

规则：
1. 用户描述要做的一件事时，识别为 task，并拆成 3-7 个执行步骤。
2. 用户明确提到目标、O、OKR、季度方向或想达成的结果时，识别为 objective。
3. 用户明确要求给某个目标创建、补充或拆解 KR 时，识别为 key_result。
4. O 描述方向和价值；KR 描述可验证结果；任务描述具体行动。
5. 创建任务时，只能从提供的已有目标和 KR 中选择归属；不能确定时省略 objectiveTitle 和 keyResultTitle，不要编造。
6. 不编造用户没有提供的精确业务数字。
7. 只返回 JSON。`;

function stringValue(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}

function titledItemsValue(value: unknown, limit: number): { title: string }[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap(item => {
    if (!item || typeof item !== 'object') return [];
    const title = stringValue((item as Record<string, unknown>).title);
    return title ? [{ title }] : [];
  }).slice(0, limit);
}

export function parseWorkbenchResponse(content: string): WorkbenchAiDraft {
  const json = content.match(/\{[\s\S]*}/)?.[0];
  if (!json) throw new Error('AI 返回格式不正确，未找到 JSON');
  const parsed = JSON.parse(json) as Record<string, unknown>;

  if (parsed.intent === 'objective') {
    const title = stringValue(parsed.title);
    if (!title) throw new Error('AI 未返回有效的目标标题');
    return {
      intent: 'objective',
      title,
      description: stringValue(parsed.description),
      keyResults: titledItemsValue(parsed.keyResults, 5),
    };
  }

  if (parsed.intent === 'key_result') {
    const keyResults = titledItemsValue(parsed.keyResults, 5);
    if (keyResults.length === 0) throw new Error('AI 未返回有效的 KR');
    return {
      intent: 'key_result',
      objectiveTitle: stringValue(parsed.objectiveTitle),
      keyResults,
    };
  }

  const title = stringValue(parsed.title);
  if (!title) throw new Error('AI 未返回有效的任务标题');
  const priority = parsed.priority === 'high' || parsed.priority === 'low' ? parsed.priority : 'medium';
  return {
    intent: 'task',
    title,
    notes: stringValue(parsed.notes) ?? `围绕「${title}」明确目标、执行步骤和预期产出。`,
    priority,
    dueDate: typeof parsed.dueDate === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(parsed.dueDate) ? parsed.dueDate : undefined,
    objectiveTitle: stringValue(parsed.objectiveTitle),
    keyResultTitle: stringValue(parsed.keyResultTitle),
    subTasks: titledItemsValue(parsed.subTasks, 7),
  };
}

export async function parseWorkbenchInput(description: string, context: { availableGoals?: Array<{ title: string; keyResults: Array<{ code: string; title: string }> }> } = {}): Promise<WorkbenchAiDraft> {
  const fallback = getModels()[0];
  const model = getFeatureModel('task') || (fallback ? modelSelectionKey(fallback) : '');
  if (!model) throw new Error('未配置 AI 模型');

  const now = new Date();
  const contextText = [
    `今天是 ${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`,
    context.availableGoals?.length
      ? `已有目标与 KR：\n${context.availableGoals.map(goal => `- O：${goal.title}${goal.keyResults.length ? `\n  ${goal.keyResults.map(result => `${result.code}：${result.title}`).join('\n  ')}` : ''}`).join('\n')}`
      : '当前没有已有目标。',
  ].filter(Boolean).join('\n');

  const response = await callLlm({
    model,
    messages: [{ role: 'user', content: `${contextText}\n\n用户输入：${description}` }],
    systemPrompt: SYSTEM_PROMPT,
    temperature: 0.5,
    feature: 'workbench-input',
  });

  return parseWorkbenchResponse(response.content);
}
