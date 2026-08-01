import { getBackendUrl } from '@/core/config/backendUrl';

export type ImageCopyOutcome = 'copied' | 'downloaded_and_url_copied' | 'downloaded';

function absoluteUrl(source: string): string {
  try {
    return new URL(source, window.location.href).href;
  } catch {
    return source;
  }
}

function isRemoteCrossOrigin(source: string): boolean {
  try {
    const url = new URL(source, window.location.href);
    return /^https?:$/.test(url.protocol) && url.origin !== window.location.origin;
  } catch {
    return false;
  }
}

export async function fetchImageBlob(source: string): Promise<Blob> {
  const response = isRemoteCrossOrigin(source)
    ? await fetch(`${getBackendUrl()}/api/images/fetch`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ url: absoluteUrl(source) }),
      })
    : await fetch(source);

  if (!response.ok) {
    const payload = await response.json().catch(() => null) as { error?: string } | null;
    throw new Error(payload?.error || `图片读取失败（${response.status}）`);
  }
  const blob = await response.blob();
  if (!blob.type.startsWith('image/')) throw new Error('返回内容不是图片');
  return blob;
}

async function loadImage(blob: Blob): Promise<HTMLImageElement> {
  const objectUrl = URL.createObjectURL(blob);
  try {
    const image = new Image();
    image.decoding = 'async';
    image.src = objectUrl;
    await image.decode();
    return image;
  } finally {
    URL.revokeObjectURL(objectUrl);
  }
}

export async function convertImageToPng(blob: Blob): Promise<Blob> {
  if (blob.type === 'image/png') return blob;
  const image = await loadImage(blob);
  const canvas = document.createElement('canvas');
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('浏览器无法创建图片画布');
  context.drawImage(image, 0, 0);
  return new Promise<Blob>((resolve, reject) => {
    canvas.toBlob(result => {
      if (result) resolve(result);
      else reject(new Error('PNG 转换失败'));
    }, 'image/png');
  });
}

export function downloadBlob(blob: Blob, filename = 'generated-image.png'): void {
  const objectUrl = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = objectUrl;
  anchor.download = filename;
  anchor.rel = 'noopener';
  anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(objectUrl), 0);
}

export async function downloadImage(source: string, filename?: string): Promise<void> {
  const png = await convertImageToPng(await fetchImageBlob(source));
  downloadBlob(png, filename);
}

export async function copyImage(source: string): Promise<ImageCopyOutcome> {
  const png = await convertImageToPng(await fetchImageBlob(source));
  try {
    if (!window.ClipboardItem || !navigator.clipboard?.write) {
      throw new Error('当前浏览器不支持图片剪贴板');
    }
    await navigator.clipboard.write([new ClipboardItem({ 'image/png': png })]);
    return 'copied';
  } catch {
    downloadBlob(png);
    try {
      await navigator.clipboard.writeText(absoluteUrl(source));
      return 'downloaded_and_url_copied';
    } catch {
      return 'downloaded';
    }
  }
}


