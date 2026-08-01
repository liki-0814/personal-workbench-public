import { useState, useCallback } from 'react';
import type { EditorTab } from '../types';
import { showToast } from '@/shell';
import { readTextFile, writeTextFile } from '../api';

const EXT_LANGUAGE_MAP: Record<string, string> = {
  '.ts': 'typescript', '.tsx': 'typescript',
  '.js': 'javascript', '.jsx': 'javascript',
  '.json': 'json', '.jsonc': 'json',
  '.md': 'markdown', '.mdx': 'markdown',
  '.css': 'css', '.scss': 'scss', '.less': 'less',
  '.html': 'html', '.htm': 'html',
  '.xml': 'xml', '.svg': 'xml',
  '.py': 'python',
  '.rs': 'rust',
  '.go': 'go',
  '.java': 'java',
  '.sql': 'sql',
  '.sh': 'shell', '.bash': 'shell', '.zsh': 'shell',
  '.yml': 'yaml', '.yaml': 'yaml',
  '.toml': 'ini',
  '.dockerfile': 'dockerfile',
  '.graphql': 'graphql',
  '.lua': 'lua',
  '.rb': 'ruby',
  '.swift': 'swift',
  '.kt': 'kotlin',
  '.c': 'c', '.h': 'c',
  '.cpp': 'cpp', '.hpp': 'cpp', '.cc': 'cpp',
};

export function detectLanguage(filename: string): string {
  if (filename === 'Dockerfile') return 'dockerfile';
  if (filename === 'Makefile') return 'makefile';
  const ext = '.' + filename.split('.').pop()?.toLowerCase();
  return EXT_LANGUAGE_MAP[ext] || 'plaintext';
}

const STORAGE_KEY = 'pwb_studio_root';

function loadRootDir(): string {
  try {
    return localStorage.getItem(STORAGE_KEY) || '';
  } catch { return ''; }
}

export function useStudio() {
  const [rootDir, setRootDirState] = useState(loadRootDir);
  const [tabs, setTabs] = useState<EditorTab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);

  const setRootDir = useCallback((dir: string) => {
    setRootDirState(dir);
    // eslint-disable-next-line no-restricted-syntax -- local-only UI state
    localStorage.setItem(STORAGE_KEY, dir);
    setTabs([]);
    setActiveTabId(null);
  }, []);

  const openFile = useCallback(async (filePath: string) => {
    const existing = tabs.find(t => t.path === filePath);
    if (existing) {
      setActiveTabId(existing.id);
      return;
    }
    try {
      const data = await readTextFile(filePath);
      if (data.isBinary) {
        showToast({ message: 'Studio 暂不编辑二进制文件', type: 'info' });
        return;
      }

      const name = filePath.split('/').pop() || filePath;
      const tab: EditorTab = {
        id: filePath,
        path: filePath,
        name,
        content: data.content,
        originalContent: data.content,
        language: detectLanguage(name),
        dirty: false,
      };
      setTabs(prev => [...prev, tab]);
      setActiveTabId(filePath);
    } catch (error) {
      showToast({ message: error instanceof Error ? error.message : '无法打开文件', type: 'error' });
    }
  }, [tabs]);

  const closeTab = useCallback((id: string) => {
    setTabs(prev => {
      const next = prev.filter(t => t.id !== id);
      if (activeTabId === id) {
        const idx = prev.findIndex(t => t.id === id);
        const newActive = next[Math.min(idx, next.length - 1)]?.id ?? null;
        setActiveTabId(newActive);
      }
      return next;
    });
  }, [activeTabId]);

  const setContent = useCallback((id: string, content: string) => {
    setTabs(prev => prev.map(t =>
      t.id === id ? { ...t, content, dirty: content !== t.originalContent } : t
    ));
  }, []);

  const saveFile = useCallback(async (id: string) => {
    const tab = tabs.find(t => t.id === id);
    if (!tab || !tab.dirty) return;
    try {
      await writeTextFile(tab.path, tab.content);
      setTabs(prev => prev.map(t =>
        t.id === id ? { ...t, originalContent: t.content, dirty: false } : t
      ));
      showToast({ message: `已保存 ${tab.name}`, type: 'success' });
    } catch (error) {
      showToast({ message: error instanceof Error ? error.message : '保存失败', type: 'error' });
    }
  }, [tabs]);

  const activeTab = tabs.find(t => t.id === activeTabId) ?? null;

  return {
    rootDir, setRootDir,
    tabs, activeTab, activeTabId,
    setActiveTabId, openFile, closeTab, setContent, saveFile,
  };
}
