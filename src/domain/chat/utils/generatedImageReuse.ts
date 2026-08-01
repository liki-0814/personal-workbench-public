import type { ChatMessage, GeneratedImageRecord } from '../types';

const EDIT_ACTION = /(?:修改|修正|改(?:成|为|一下)?|调整|替换|移除|删除|去掉|增加|添加|补充|缩小|放大|移动|fix|edit|change|replace|remove|add|adjust)/i;
const IMAGE_TARGET = /(?:图片|图像|这张图|原图|标题|文字|文案|字体|颜色|配色|布局|构图|模块|图标|箭头|标签|水印|logo|image|picture|title|text|color|layout|composition)/i;
const IMAGE_ISSUE = /(?:错字|乱码|错误|重复|缺失|不对|模糊|太大|太小|歪|畸形|wrong|typo|garbled|missing|blurry)/i;

function extractImageEditInstruction(content: string): string | undefined {
  const trimmed = content.trim();
  if (!trimmed) return undefined;

  const targetedMarker = trimmed.match(/只修改\s*[:：]/);
  if (targetedMarker?.index !== undefined) {
    const remainder = trimmed.slice(targetedMarker.index + targetedMarker[0].length);
    const targeted = remainder.split(/\n+\s*(?:其余|其他)/, 1)[0].trim();
    return targeted || undefined;
  }

  if (!(EDIT_ACTION.test(trimmed) && IMAGE_TARGET.test(trimmed))
    && !(IMAGE_ISSUE.test(trimmed) && IMAGE_TARGET.test(trimmed))) {
    return undefined;
  }

  return trimmed.length > 800 ? `${trimmed.slice(0, 800)}…` : trimmed;
}

export function findLatestGeneratedImageEditInstruction(
  messages: ChatMessage[],
  sourceMessageIndex: number,
): string | undefined {
  for (let index = messages.length - 1; index > sourceMessageIndex; index -= 1) {
    const message = messages[index];
    if (message.role !== 'user') continue;
    const instruction = extractImageEditInstruction(message.content);
    if (instruction) return instruction;
  }
  return undefined;
}

export function buildGeneratedImageReusePrompt(
  record: GeneratedImageRecord,
  mode: 'edit' | 'variant',
  historyInstruction?: string,
): string {
  if (mode === 'variant') {
    return '请基于附带的原图生成一个相似版本。将图片引用作为 style 和 composition 参考，保持核心主体与整体风格一致，并在构图细节上做合理变化。';
  }

  const requestedChange = historyInstruction?.trim()
    || record.qa.repairPrompt?.trim()
    || record.qa.issues.map(issue => issue.description.trim()).filter(Boolean).join('；')
    || '根据原始 Visual Brief 优化图像质量';

  return `请基于附带的原图进行局部修改，并将图片引用作为 edit-target。\n\n只修改：${requestedChange}\n\n其余内容、构图、配色与风格保持不变。`;
}
