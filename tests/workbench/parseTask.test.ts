import { describe, expect, it, vi } from 'vitest';

const { callLlm, getFeatureModel, getModels } = vi.hoisted(() => ({
  callLlm: vi.fn(),
  getFeatureModel: vi.fn(() => 'test-model'),
  getModels: vi.fn(() => [{ id: 'fallback-model' }]),
}));

vi.mock('@/core/llm', () => ({ callLlm }));
vi.mock('@/core/config', () => ({ getFeatureModel, getModels }));

import { parseTaskWithAI, TASK_PARSE_SYSTEM_PROMPT } from '@/domain/todo/ai/parseTask';

describe('parseTaskWithAI', () => {
  it('parses a complete AI response', async () => {
    callLlm.mockResolvedValue({
      content: JSON.stringify({
        title: '写周报',
        notes: '总结本周工作进展',
        priority: 'medium',
        dueDate: '2026-06-20',
        subTasks: [{ title: '收集数据' }, { title: '撰写总结' }],
      }),
    });

    const result = await parseTaskWithAI('写个周报');

    expect(result.title).toBe('写周报');
    expect(result.notes).toBe('总结本周工作进展');
    expect(result.priority).toBe('medium');
    expect(result.dueDate).toBe('2026-06-20');
    expect(result.subTasks).toEqual([{ title: '收集数据' }, { title: '撰写总结' }]);
  });

  it('defaults priority to medium when invalid', async () => {
    callLlm.mockResolvedValue({
      content: JSON.stringify({ title: '任务', notes: '', priority: 'urgent', subTasks: [] }),
    });
    const result = await parseTaskWithAI('test');
    expect(result.priority).toBe('medium');
  });

  it('generates fallback notes when empty', async () => {
    callLlm.mockResolvedValue({
      content: JSON.stringify({ title: '部署', notes: '', subTasks: [] }),
    });
    const result = await parseTaskWithAI('test');
    expect(result.notes).toContain('部署');
  });

  it('ignores invalid dueDate format', async () => {
    callLlm.mockResolvedValue({
      content: JSON.stringify({ title: '任务', notes: 'n', dueDate: 'tomorrow', subTasks: [] }),
    });
    const result = await parseTaskWithAI('test');
    expect(result.dueDate).toBeUndefined();
  });

  it('filters out invalid subTasks', async () => {
    callLlm.mockResolvedValue({
      content: JSON.stringify({
        title: '任务', notes: 'n', subTasks: [
          { title: '有效步骤' },
          { title: '' },
          null,
          'invalid',
          { title: '另一个步骤' },
        ],
      }),
    });
    const result = await parseTaskWithAI('test');
    expect(result.subTasks).toEqual([{ title: '有效步骤' }, { title: '另一个步骤' }]);
  });

  it('extracts JSON from markdown-wrapped response', async () => {
    callLlm.mockResolvedValue({
      content: '```json\n{"title":"任务","notes":"备注","subTasks":[]}\n```',
    });
    const result = await parseTaskWithAI('test');
    expect(result.title).toBe('任务');
  });

  it('throws when response contains no JSON', async () => {
    callLlm.mockResolvedValue({ content: '抱歉，我无法理解您的请求' });
    await expect(parseTaskWithAI('test')).rejects.toThrow('未找到 JSON');
  });

  it('throws when title is empty', async () => {
    callLlm.mockResolvedValue({
      content: JSON.stringify({ title: '', notes: 'n', subTasks: [] }),
    });
    await expect(parseTaskWithAI('test')).rejects.toThrow('有效的任务标题');
  });

  it('throws when no model is configured', async () => {
    getFeatureModel.mockReturnValueOnce('');
    getModels.mockReturnValueOnce([]);
    await expect(parseTaskWithAI('test')).rejects.toThrow('未配置 AI 模型');
  });

  it('passes system prompt with date context to callLlm', async () => {
    callLlm.mockResolvedValue({
      content: JSON.stringify({ title: '任务', notes: 'n', subTasks: [] }),
    });
    await parseTaskWithAI('做个任务');

    expect(callLlm).toHaveBeenCalledWith(
      expect.objectContaining({
        model: 'test-model',
        messages: [{ role: 'user', content: '做个任务' }],
        systemPrompt: expect.stringContaining('智能任务拆解'),
        temperature: 0.7,
      }),
    );
  });

  it('TASK_PARSE_SYSTEM_PROMPT includes JSON schema instructions', () => {
    expect(TASK_PARSE_SYSTEM_PROMPT).toContain('"title"');
    expect(TASK_PARSE_SYSTEM_PROMPT).toContain('"subTasks"');
    expect(TASK_PARSE_SYSTEM_PROMPT).toContain('"priority"');
  });
});
