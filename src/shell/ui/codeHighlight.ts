import hljs from 'highlight.js/lib/core';
import type { LanguageFn } from 'highlight.js';
import bash from 'highlight.js/lib/languages/bash';
import cpp from 'highlight.js/lib/languages/cpp';
import csharp from 'highlight.js/lib/languages/csharp';
import css from 'highlight.js/lib/languages/css';
import go from 'highlight.js/lib/languages/go';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import markdown from 'highlight.js/lib/languages/markdown';
import python from 'highlight.js/lib/languages/python';
import rust from 'highlight.js/lib/languages/rust';
import sql from 'highlight.js/lib/languages/sql';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';

const LANGUAGE_DEFINITIONS: Record<string, LanguageFn> = {
  bash,
  cpp,
  csharp,
  css,
  go,
  java,
  javascript,
  json,
  markdown,
  python,
  rust,
  sql,
  typescript,
  xml,
  yaml,
};

const LANGUAGE_ALIASES: Record<string, string> = {
  'c++': 'cpp',
  cs: 'csharp',
  html: 'xml',
  js: 'javascript',
  jsx: 'javascript',
  md: 'markdown',
  mjs: 'javascript',
  py: 'python',
  rs: 'rust',
  sh: 'bash',
  shell: 'bash',
  ts: 'typescript',
  tsx: 'typescript',
  vue: 'xml',
  yml: 'yaml',
  zsh: 'bash',
};

const REGISTERED_LANGUAGES = Object.keys(LANGUAGE_DEFINITIONS);

for (const [name, definition] of Object.entries(LANGUAGE_DEFINITIONS)) {
  if (!hljs.getLanguage(name)) hljs.registerLanguage(name, definition);
}

export function normalizeCodeLanguage(language?: string): string | undefined {
  const raw = language?.trim().toLowerCase().split(/\s+/)[0];
  if (!raw) return undefined;
  return LANGUAGE_ALIASES[raw] ?? raw;
}

export function escapeCodeHtml(code: string): string {
  return code
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}

export function highlightCode(code: string, language?: string): { html: string; language?: string } {
  const normalized = normalizeCodeLanguage(language);

  try {
    if (normalized && hljs.getLanguage(normalized)) {
      const result = hljs.highlight(code, { language: normalized, ignoreIllegals: true });
      return { html: result.value, language: normalized };
    }

    const result = hljs.highlightAuto(code, REGISTERED_LANGUAGES);
    return { html: result.value, language: result.language };
  } catch {
    return { html: escapeCodeHtml(code), language: normalized };
  }
}
