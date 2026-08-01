import type { ClipboardEvent } from 'react';

/** 拦截剪贴板里的图片，把 dataURL 通过回调传出。
 *  调用方决定要插入什么 markdown（直链或短引用）。 */
export function handleImagePaste(
  e: ClipboardEvent<HTMLTextAreaElement>,
  onImage: (dataUrl: string) => void,
): boolean {
  const items = e.clipboardData?.items;
  if (!items) return false;

  const images: File[] = [];
  for (const item of items) {
    if (item.type.startsWith('image/')) {
      const file = item.getAsFile();
      if (file) images.push(file);
    }
  }
  if (images.length === 0) return false;

  e.preventDefault();
  for (const file of images) {
    const reader = new FileReader();
    reader.onload = () => {
      onImage(reader.result as string);
    };
    reader.readAsDataURL(file);
  }
  return true;
}

export function insertAtCursor(
  textarea: HTMLTextAreaElement,
  text: string,
  currentValue: string,
  setValue: (v: string) => void,
): void {
  const start = textarea.selectionStart ?? currentValue.length;
  const end = textarea.selectionEnd ?? currentValue.length;
  const next = currentValue.slice(0, start) + text + currentValue.slice(end);
  setValue(next);
  requestAnimationFrame(() => {
    const pos = start + text.length;
    textarea.setSelectionRange(pos, pos);
    textarea.focus();
  });
}

const DATA_URL_IMG_RE = /!\[([^\]]*)\]\((data:image\/[^)]+)\)/g;
const SHORT_REF_RE = /pwb-img:\/\/([a-z0-9-]+)/gi;

/** 把 markdown 里的 base64 图片压缩成短引用 `pwb-img://refId`。
 *  返回压缩后内容 + 引用 → dataURL 的映射。
 *  传入 existing 可以复用已有 ID（避免同一 dataURL 注册多次）。 */
export function compactBase64Images(
  content: string,
  existing?: Map<string, string>,
): { compact: string; map: Map<string, string> } {
  const map = new Map(existing);
  const dataUrlToRef = new Map<string, string>();
  for (const [refId, url] of map) dataUrlToRef.set(url, refId);

  const compact = content.replace(DATA_URL_IMG_RE, (_match, alt: string, dataUrl: string) => {
    let refId = dataUrlToRef.get(dataUrl);
    if (!refId) {
      refId = `img-${Math.random().toString(36).slice(2, 10)}`;
      map.set(refId, dataUrl);
      dataUrlToRef.set(dataUrl, refId);
    }
    return `![${alt}](pwb-img://${refId})`;
  });
  return { compact, map };
}

/** 把短引用还原回 base64 dataURL。未找到的引用保持原样。 */
export function expandBase64Images(content: string, map: Map<string, string>): string {
  return content.replace(SHORT_REF_RE, (full, refId: string) => map.get(refId) || full);
}
