import { useState, useCallback, useEffect, useRef } from 'react';
import { load, save, KEYS, useStorageSync } from '@/core/storage';
import { generateId } from '@/core/utils/id';
import { now } from '@/core/utils/date';
import type { ClipboardItem, ClipboardItemType } from '../types';

const MAX_HISTORY = 50;

async function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onloadend = () => resolve(reader.result as string);
    reader.onerror = reject;
    reader.readAsDataURL(blob);
  });
}

const URL_REGEX = /^(https?:\/\/)?([\da-z.-]+)\.([a-z.]{2,6})([/\w .-]*)*\/?.*$/i;

function detectType(content: string): ClipboardItemType {
  // URL detection
  if (URL_REGEX.test(content.trim())) {
    return 'url';
  }
  // Code heuristics
  const codePatterns = [
    /\{[\s\S]*?\}/,           // curly braces block
    /;\s*$/,                    // semicolon at end of line
    /\bfunction\b/,             // function keyword
    /=>\s*\{?/,                 // arrow function
    /\bconst\s+\w+\s*=/,       // const declaration
    /\blet\s+\w+\s*=/,         // let declaration
    /\bvar\s+\w+\s*=/,         // var declaration
    /\bclass\s+\w+/,           // class declaration
    /\bimport\s+.*\bfrom\b/,   // import statement
    /\bexport\s+(default\s+)?/, // export statement
    /\breturn\s+/,              // return statement
    /<\w+.*?>.*<\/\w+>/,       // HTML-like tags
    /\(\s*\)\s*=>/,            // arrow function shorthand
    /`[\s\S]*?`/,              // template literal
  ];
  const isCode = codePatterns.some(p => p.test(content));
  if (isCode) {
    return 'code';
  }
  return 'text';
}

export function useClipboardHistory() {
  const [history, setHistory] = useState<ClipboardItem[]>(() => {
    const data = load<ClipboardItem[]>(KEYS.CLIPBOARD, []);
    return data.slice(0, MAX_HISTORY);
  });
  const lastImageRef = useRef<string>('');

  useStorageSync(() => {
    setHistory(load<ClipboardItem[]>(KEYS.CLIPBOARD, []).slice(0, MAX_HISTORY));
  });

  const persist = useCallback((items: ClipboardItem[]) => {
    const trimmed = items.slice(0, MAX_HISTORY);
    setHistory(trimmed);
    save(KEYS.CLIPBOARD, trimmed);
  }, []);

  const addItem = useCallback((content: string, source?: string) => {
    if (!content || !content.trim()) return;
    const trimmed = content.trim();

    setHistory(prev => {
      // Skip if identical to most recent item
      if (prev.length > 0 && prev[0].content === trimmed) {
        return prev;
      }
      const item: ClipboardItem = {
        id: generateId(),
        content: trimmed,
        type: detectType(trimmed),
        timestamp: now(),
        source,
      };
      const next = [item, ...prev].slice(0, MAX_HISTORY);
      save(KEYS.CLIPBOARD, next);
      return next;
    });
  }, []);

  const addImageItem = useCallback((base64: string, source?: string) => {
    if (!base64 || base64 === lastImageRef.current) return;
    lastImageRef.current = base64;

    setHistory(prev => {
      const item: ClipboardItem = {
        id: generateId(),
        content: base64,
        type: 'image',
        timestamp: now(),
        source,
      };
      const next = [item, ...prev].slice(0, MAX_HISTORY);
      save(KEYS.CLIPBOARD, next);
      return next;
    });
  }, []);

  const removeItem = useCallback((id: string) => {
    setHistory(prev => {
      const next = prev.filter(item => item.id !== id);
      save(KEYS.CLIPBOARD, next);
      return next;
    });
  }, []);

  const clear = useCallback(() => {
    persist([]);
  }, [persist]);

  const copyToClipboard = useCallback(async (content: string, type?: ClipboardItemType): Promise<boolean> => {
    try {
      if (type === 'image' && content.startsWith('data:')) {
        const res = await fetch(content);
        const blob = await res.blob();
        await navigator.clipboard.write([
          new ClipboardItem({ [blob.type]: blob }),
        ]);
        return true;
      }
      await navigator.clipboard.writeText(content);
      return true;
    } catch (error) {
      console.warn('Failed to write to clipboard:', error);
      return false;
    }
  }, []);

  // Monitor clipboard changes on focus/visibility change
  useEffect(() => {
    let isMounted = true;

    const checkClipboard = async () => {
      if (!isMounted) return;
      try {
        // Try reading images first (requires navigator.clipboard.read)
        if (navigator.clipboard.read) {
          const items = await navigator.clipboard.read();
          for (const clipboardItem of items) {
            const imageType = clipboardItem.types.find(t => t.startsWith('image/'));
            if (imageType) {
              const blob = await clipboardItem.getType(imageType);
              const base64 = await blobToBase64(blob);
              addImageItem(base64);
              return; // Image handled, skip text
            }
          }
        }
        // Fall back to text
        const text = await navigator.clipboard.readText();
        if (text && text.trim()) {
          addItem(text.trim());
        }
      } catch {
        // Permission denied or not supported — silently ignore
      }
    };

    const handleFocus = () => {
      checkClipboard();
    };

    const handleVisibilityChange = () => {
      if (document.visibilityState === 'visible') {
        checkClipboard();
      }
    };

    const handlePaste = async (e: ClipboardEvent) => {
      if (!isMounted) return;
      const items = e.clipboardData?.items;
      if (!items) return;

      for (const item of Array.from(items)) {
        if (item.type.startsWith('image/')) {
          e.preventDefault();
          const blob = item.getAsFile();
          if (blob) {
            const base64 = await blobToBase64(blob);
            addImageItem(base64, '粘贴');
          }
          return;
        }
      }

      // Text paste – let it go through and read via readText if needed
      const text = e.clipboardData?.getData('text');
      if (text && text.trim()) {
        addItem(text.trim(), '粘贴');
      }
    };

    // Initial check
    checkClipboard();

    window.addEventListener('focus', handleFocus);
    document.addEventListener('visibilitychange', handleVisibilityChange);
    document.addEventListener('paste', handlePaste);

    return () => {
      isMounted = false;
      window.removeEventListener('focus', handleFocus);
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      document.removeEventListener('paste', handlePaste);
    };
  }, [addItem, addImageItem]);

  return {
    history,
    addItem,
    addImageItem,
    removeItem,
    clear,
    copyToClipboard,
  };
}
