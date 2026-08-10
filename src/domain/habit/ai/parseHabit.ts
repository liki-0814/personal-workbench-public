import { callLlm } from '@/core/llm';
import { getModels, getFeatureModel, modelSelectionKey } from '@/core/config';
import type { HabitFrequency } from '../types';

export interface ParsedHabit {
  title: string;
  emoji: string;
  frequency: HabitFrequency;
}

function isValidFrequency(value: unknown): value is HabitFrequency {
  if (typeof value !== 'object' || value === null) return false;
  const freq = value as Record<string, unknown>;
  switch (freq.type) {
    case 'daily':
      return true;
    case 'weekly':
      return (
        typeof freq.timesPerWeek === 'number' &&
        Number.isInteger(freq.timesPerWeek) &&
        freq.timesPerWeek >= 1 &&
        freq.timesPerWeek <= 7
      );
    case 'weekdays':
      return (
        Array.isArray(freq.days) &&
        freq.days.length > 0 &&
        freq.days.every(d => Number.isInteger(d) && d >= 0 && d <= 6)
      );
    default:
      return false;
  }
}

function isParsedHabit(item: unknown): item is ParsedHabit {
  if (typeof item !== 'object' || item === null) return false;
  const candidate = item as Record<string, unknown>;
  return (
    typeof candidate.title === 'string' &&
    candidate.title.trim().length > 0 &&
    typeof candidate.emoji === 'string' &&
    isValidFrequency(candidate.frequency)
  );
}

export async function parseHabitWithAI(goal: string): Promise<ParsedHabit[]> {
  const systemPrompt = `你是一个习惯养成顾问。用户会描述一个目标，你需要将其拆解为具体的、可执行的日常习惯。

要求：
- 每个习惯必须是具体的、可打卡的行为（不是模糊的目标）
- 为每个习惯选择一个合适的 emoji
- 根据习惯性质设置合理的频率
- 返回 3-6 个习惯

返回 JSON 数组，格式：
[{"title": "习惯名称", "emoji": "🏃", "frequency": {"type": "daily"}}]

频率类型：
- {"type": "daily"} — 每天
- {"type": "weekly", "timesPerWeek": 3} — 每周N次
- {"type": "weekdays", "days": [1,3,5]} — 指定星期（0=周日,1=周一...6=周六）

只返回 JSON 数组，不要其他文字。`;

  const fallback = getModels()[0];
  const model = getFeatureModel('task') || (fallback ? modelSelectionKey(fallback) : '');
  if (!model) throw new Error('未配置 AI 模型');

  const response = await callLlm({
    model,
    messages: [{ role: 'user', content: `我的目标：${goal}` }],
    systemPrompt,
    temperature: 0.7,
    feature: 'habit-parse',
  });

  const text = response.content || '';
  const jsonMatch = text.match(/\[[\s\S]*\]/);
  if (!jsonMatch) return [];

  try {
    const parsed: unknown = JSON.parse(jsonMatch[0]);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isParsedHabit);
  } catch {
    return [];
  }
}
