import DOMPurify from 'dompurify';
import { documentAssetUrl } from '../api';

const ASSET_ATTRIBUTE = 'data-pwb-asset';
const EDITOR_STYLE_ATTRIBUTE = 'data-pwb-editor-style';

export function prepareHtmlForFrame(html: string, documentId: string): string {
  const sanitized = DOMPurify.sanitize(html, {
    WHOLE_DOCUMENT: true,
    FORBID_TAGS: ['script', 'iframe', 'object', 'embed'],
    FORBID_ATTR: ['srcdoc'],
  });
  const parsed = new DOMParser().parseFromString(sanitized, 'text/html');
  parsed.querySelectorAll<HTMLElement>('[src], [poster]').forEach(element => {
    for (const attribute of ['src', 'poster']) {
      const value = element.getAttribute(attribute);
      if (!value?.startsWith('assets/')) continue;
      element.setAttribute(ASSET_ATTRIBUTE, value);
      element.setAttribute(attribute, documentAssetUrl(documentId, value));
    }
  });
  parsed.querySelectorAll('style').forEach(style => {
    style.textContent = rewriteCssAssets(style.textContent || '', documentId);
  });
  const editorStyle = parsed.createElement('style');
  editorStyle.setAttribute(EDITOR_STYLE_ATTRIBUTE, 'true');
  editorStyle.textContent = `
    [contenteditable="true"] { caret-color: #2563eb; }
    [contenteditable="true"]:focus { outline: none; }
    [contenteditable="true"] [data-pwb-selected] { outline: 2px solid #2563eb; outline-offset: 2px; }
  `;
  parsed.head.append(editorStyle);
  return `<!doctype html>\n${parsed.documentElement.outerHTML}`;
}

export function serializeFrameDocument(document: Document, documentId: string): string {
  const clone = document.documentElement.cloneNode(true) as HTMLElement;
  clone.querySelectorAll(`[${EDITOR_STYLE_ATTRIBUTE}]`).forEach(element => element.remove());
  clone.querySelectorAll<HTMLElement>(`[${ASSET_ATTRIBUTE}]`).forEach(element => {
    const asset = element.getAttribute(ASSET_ATTRIBUTE);
    if (asset) {
      if (element.hasAttribute('src')) element.setAttribute('src', asset);
      if (element.hasAttribute('poster')) element.setAttribute('poster', asset);
    }
    element.removeAttribute(ASSET_ATTRIBUTE);
    element.removeAttribute('contenteditable');
    element.removeAttribute('data-pwb-selected');
  });
  clone.querySelectorAll('[contenteditable]').forEach(element => element.removeAttribute('contenteditable'));
  const assetPrefix = documentAssetUrl(documentId, '');
  clone.querySelectorAll('style').forEach(style => {
    style.textContent = (style.textContent || '').split(assetPrefix).join('assets/');
  });
  return `<!doctype html>\n${clone.outerHTML}`;
}

function rewriteCssAssets(css: string, documentId: string): string {
  return css.replace(
    /url\(\s*(['"]?)(assets\/[A-Za-z0-9._-]+)\1\s*\)/g,
    (_match, quote: string, asset: string) => `url(${quote}${documentAssetUrl(documentId, asset)}${quote})`,
  );
}
