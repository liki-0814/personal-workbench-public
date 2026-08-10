import { callLlm } from '@/core/llm';
import { getModels, getFeatureModel, modelSelectionKey } from '@/core/config';

export const TASK_PARSE_SYSTEM_PROMPT = `你是一个智能任务拆解助手。用户会用自然语言描述一件事，你需要将其拆解成结构化的任务数据。

## 输出格式
你必须返回一个 JSON 对象，格式如下：
{
  "title": "主任务标题（简洁明了，10字以内）",
  "notes": "备注：概括任务背景、目标、产出物或注意事项",
  "priority": "high|medium|low",
  "dueDate": "YYYY-MM-DD（可选，如果用户提到了时间；今天/今日也要填写当天日期）",
  "subTasks": [
    { "title": "执行项1" },
    { "title": "执行项2" }
  ]
}

## 规则
1. title 必须简洁，直接描述任务核心
2. notes 必须填写，说明任务背景、目标、预期产出或注意事项；如果信息有限，基于标题生成一句有用备注
3. priority 推断标准：
   - high: 紧急、重要、有明确截止时间且时间紧迫
   - medium: 一般性工作，有一定时间要求
   - low: 不紧急、可随时做的琐事
4. dueDate 仅在用户明确提到日期时填写（如"今天"、"明天"、"周五"、"4月25日"）
5. subTasks 是任务下的执行项，必须将用户描述的操作拆解成3-7个具体步骤；不要把 O 或 KR 本身作为执行项
7. 不要返回任何 JSON 以外的内容
8. 如果无法拆解，subTasks 可以为空数组`;

export interface ParsedTask {
  title: string;
  notes: string;
  priority: 'high' | 'medium' | 'low';
  dueDate?: string;
  subTasks: { title: string }[];
}

export interface TaskParseContext {
  objectiveTitle?: string;
  keyResultTitle?: string;
}

export async function parseTaskWithAI(description: string, context: TaskParseContext = {}): Promise<ParsedTask> {
  const fallback = getModels()[0];
  const model = getFeatureModel('task') || (fallback ? modelSelectionKey(fallback) : '');
  if (!model) throw new Error('未配置 AI 模型');

  const today = new Date();
  const dateContext = `\n\n## 当前时间\n今天是 ${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, '0')}-${String(today.getDate()).padStart(2, '0')}（${['周日','周一','周二','周三','周四','周五','周六'][today.getDay()]}），现在是 ${String(today.getHours()).padStart(2, '0')}:${String(today.getMinutes()).padStart(2, '0')}。请据此推算"明天"、"后天"、"下周一"等相对日期。`;
  const workContext = context.keyResultTitle
    ? `\n\n## 任务归属\nO：${context.objectiveTitle ?? '未指定'}\nKR：${context.keyResultTitle}\n请围绕这个 KR 拆解当前任务，不要创建或改写 O/KR。`
    : '';

  const response = await callLlm({
    model,
    messages: [{ role: 'user', content: description }],
    systemPrompt: TASK_PARSE_SYSTEM_PROMPT + dateContext + workContext,
    temperature: 0.7,
    feature: 'task-parse',
  });

  const jsonStr = response.content.match(/\{[\s\S]*}/)?.[0];
  if (!jsonStr) throw new Error('AI 返回格式不正确，未找到 JSON');

  const parsed = JSON.parse(jsonStr) as Record<string, unknown>;
  const title = typeof parsed.title === 'string' ? parsed.title.trim() : '';
  if (!title) throw new Error('AI 未返回有效的任务标题');

  const priority: 'high' | 'medium' | 'low' = parsed.priority === 'high' || parsed.priority === 'medium' || parsed.priority === 'low'
    ? parsed.priority
    : 'medium';
  const notes = typeof parsed.notes === 'string' && parsed.notes.trim()
    ? parsed.notes.trim()
    : `围绕「${title}」明确目标、执行步骤和预期产出。`;
  const dueDate = typeof parsed.dueDate === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(parsed.dueDate)
    ? parsed.dueDate
    : undefined;
  const subTasks: { title: string }[] = [];
  if (Array.isArray(parsed.subTasks)) {
    for (const st of parsed.subTasks) {
      if (st && typeof st === 'object' && typeof (st as Record<string, unknown>).title === 'string') {
        const t = (st as Record<string, unknown>).title as string;
        if (t.trim()) subTasks.push({ title: t.trim() });
      }
    }
  }
  return { title, notes, priority, dueDate, subTasks };
}
