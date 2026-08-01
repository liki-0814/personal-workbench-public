let mermaidPromise: Promise<typeof import('mermaid').default> | null = null;

export function loadMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid').then(m => m.default);
  }
  return mermaidPromise;
}

let lastInitTheme: string | null = null;

export async function ensureMermaidInitialized(themeMode: string) {
  const mermaid = await loadMermaid();
  if (lastInitTheme !== themeMode) {
    mermaid.initialize({
      startOnLoad: false,
      theme: 'base',
      themeVariables: getMermaidThemeVariables(themeMode),
      securityLevel: 'strict',
      fontFamily: 'ui-sans-serif, system-ui, -apple-system, sans-serif',
    });
    lastInitTheme = themeMode;
  }
  return mermaid;
}

let renderCounter = 0;

export function createMermaidRenderId() {
  return `mmd-${++renderCounter}`;
}

/**
 * 模块级 SVG 缓存。Key = `${themeMode}\n${code}`。
 *
 * 流式 chat 里，react-markdown 每个 token 都重新解析 markdown 树，<Mermaid> 经常被
 * unmount + remount。如果只在组件 state 里保存渲染结果，重建就丢；用模块级 Map 即使
 * 组件死了缓存也活着，重新挂载时立即 innerHTML 回放，不再 "出图 → 消失 → 出图" 闪。
 *
 * 容量上限：FIFO 50 条。每篇大对话里通常只有 1-3 张图，远够用；超出按插入顺序丢最早的。
 */
const SVG_CACHE = new Map<string, string>();
const SVG_CACHE_LIMIT = 50;

export function cacheKey(themeMode: string, code: string) {
  return `${themeMode}\n${code}`;
}

export function cacheSet(key: string, svg: string) {
  if (SVG_CACHE.has(key)) SVG_CACHE.delete(key);
  SVG_CACHE.set(key, svg);
  if (SVG_CACHE.size > SVG_CACHE_LIMIT) {
    const oldest = SVG_CACHE.keys().next().value;
    if (oldest !== undefined) SVG_CACHE.delete(oldest);
  }
}

export function cacheGet(key: string) {
  return SVG_CACHE.get(key);
}

const DARK_THEME_VARIABLES = {
  background: 'transparent',
  primaryColor: '#2d1f5e',
  primaryTextColor: '#e2e0f0',
  primaryBorderColor: '#6366f180',
  secondaryColor: '#1a2744',
  secondaryTextColor: '#c8d6e5',
  secondaryBorderColor: '#38bdf880',
  tertiaryColor: '#2a1a3e',
  tertiaryTextColor: '#e0d4f0',
  tertiaryBorderColor: '#a855f780',
  lineColor: '#6366f1a0',
  textColor: '#e2e0f0',
  mainBkg: '#1e1b4b',
  nodeBorder: '#6366f180',
  clusterBkg: '#0f0d2408',
  clusterBorder: '#6366f140',
  titleColor: '#c4b5fd',
  edgeLabelBackground: '#1e1b4bcc',
  nodeTextColor: '#e2e0f0',
};

const LIGHT_THEME_VARIABLES = {
  background: 'transparent',
  primaryColor: '#eef2ff',
  primaryTextColor: '#312e81',
  primaryBorderColor: '#a5b4fc',
  secondaryColor: '#f0fdfa',
  secondaryTextColor: '#134e4a',
  secondaryBorderColor: '#99f6e4',
  tertiaryColor: '#faf5ff',
  tertiaryTextColor: '#581c87',
  tertiaryBorderColor: '#d8b4fe',
  lineColor: '#6366f1',
  textColor: '#1e1b4b',
  mainBkg: '#eef2ff',
  nodeBorder: '#a5b4fc',
  clusterBkg: '#f8fafc',
  clusterBorder: '#c7d2fe',
  titleColor: '#4338ca',
  edgeLabelBackground: '#ffffffee',
  nodeTextColor: '#1e1b4b',
};

export function getMermaidThemeVariables(themeMode: string) {
  return themeMode === 'dark' ? DARK_THEME_VARIABLES : LIGHT_THEME_VARIABLES;
}

export const MERMAID_CONTAINER_CLASS = 'mermaid-container my-3 flex justify-center overflow-x-auto';
export const MERMAID_ERROR_CLASS = 'markdown-pre text-red-500 dark:text-red-400 text-xs my-2 whitespace-pre-wrap';
