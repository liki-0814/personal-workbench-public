import { describe, expect, it } from 'vitest';
import type { GeneratedImageRecord } from '../types';
import type { ChatMessage } from '../types';
import { buildGeneratedImageReusePrompt, findLatestGeneratedImageEditInstruction } from './generatedImageReuse';

const record: GeneratedImageRecord = {
  id: 'image-1',
  url: '/api/images/image-1.png',
  status: 'ready_with_warnings',
  intent: 'generate',
  scene: 'scientific',
  visualBrief: {
    intent: 'generate',
    scene: 'scientific',
    prompt: '模型架构图',
    exactText: [],
    references: [],
    invariants: [],
    exclusions: [],
    factualConstraints: [],
  },
  finalPrompt: '模型架构图',
  aspectRatio: '16:9',
  modelId: 'image-model',
  providerName: 'provider',
  references: [],
  qa: {
    status: 'warning',
    issues: [{ category: 'text', description: '标题存在错字' }],
    repairPrompt: '将标题修正为“专业机器学习模型架构图”',
  },
  createdAt: '2026-07-21T00:00:00Z',
};

describe('buildGeneratedImageReusePrompt', () => {
  it('uses the QA repair prompt for targeted edits', () => {
    const prompt = buildGeneratedImageReusePrompt(record, 'edit');
    expect(prompt).toContain('作为 edit-target');
    expect(prompt).toContain('只修改：将标题修正为“专业机器学习模型架构图”');
    expect(prompt).not.toContain('只修改：\n');
  });

  it('falls back to QA issues when no repair prompt is available', () => {
    const prompt = buildGeneratedImageReusePrompt({
      ...record,
      qa: { ...record.qa, repairPrompt: undefined },
    }, 'edit');
    expect(prompt).toContain('只修改：标题存在错字');
  });

  it('prefers the latest explicit edit request after the generated image', () => {
    const messages: ChatMessage[] = [
      { role: 'user', content: '生成一张模型架构图' },
      { role: 'assistant', content: '', generatedImages: [record.url] },
      { role: 'user', content: '把标题从“专业机器学学习模型模架构图”修正为“专业机器学习模型架构图”。' },
      { role: 'assistant', content: '请重新附图。' },
      { role: 'user', content: '另外请解释一下为什么会这样。' },
    ];
    const instruction = findLatestGeneratedImageEditInstruction(messages, 1);
    const prompt = buildGeneratedImageReusePrompt(record, 'edit', instruction);
    expect(prompt).toContain('只修改：把标题从“专业机器学学习模型模架构图”修正为“专业机器学习模型架构图”。');
    expect(prompt).not.toContain(record.qa.repairPrompt!);
  });

  it('ignores empty legacy templates and unrelated conversation', () => {
    const messages: ChatMessage[] = [
      { role: 'assistant', content: '', generatedImages: [record.url] },
      { role: 'user', content: '请基于上一张生成图片进行局部修改。只修改：\n\n其余内容保持不变。' },
      { role: 'assistant', content: '请说明要修改什么。' },
      { role: 'user', content: '今天的天气怎么样？' },
    ];
    expect(findLatestGeneratedImageEditInstruction(messages, 0)).toBeUndefined();
    expect(buildGeneratedImageReusePrompt(record, 'edit')).toContain(record.qa.repairPrompt!);
  });
});
