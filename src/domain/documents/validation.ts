import type { DocumentContent, EditableDocument, HtmlDocumentContent, MarkdownDocumentContent } from './types';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function fail(path: string, expected: string): never {
  throw new Error(`文档格式损坏：${path} ${expected}`);
}

export function assertHtmlDocumentContent(value: unknown): asserts value is HtmlDocumentContent {
  if (!isRecord(value)) fail('/', '必须是对象');
  if (typeof value.html !== 'string' || !value.html.trim()) fail('/html', '必须是非空字符串');
  const lower = value.html.toLowerCase();
  if (!lower.includes('<html') || !lower.includes('<body')) {
    fail('/html', '必须包含完整的 html 和 body 元素');
  }
}

export function assertMarkdownDocumentContent(value: unknown): asserts value is MarkdownDocumentContent {
  if (!isRecord(value)) fail('/', '必须是对象');
  if (typeof value.markdown !== 'string' || !value.markdown.trim()) fail('/markdown', '必须是非空字符串');
}

export function validateEditableDocument<T extends DocumentContent>(document: EditableDocument<T>): EditableDocument<T> {
  if (document.manifest.kind === 'html') {
    assertHtmlDocumentContent(document.content);
  } else if (document.manifest.kind === 'markdown') {
    assertMarkdownDocumentContent(document.content);
  } else {
    fail('/manifest/kind', '必须是 html 或 markdown');
  }
  return document;
}
