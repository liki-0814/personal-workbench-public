import { useState, useEffect, useCallback } from 'react';
import type { ThemeConfig, ThemeMode } from '../types';
import { load, save, KEYS, useStorageSync } from '@/core/storage';

const DEFAULT_THEME: ThemeConfig = { mode: 'dark' };

export function useTheme() {
  const [theme, setTheme] = useState<ThemeConfig>(() =>
    load(KEYS.THEME, DEFAULT_THEME)
  );

  useStorageSync(() => {
    setTheme(load(KEYS.THEME, DEFAULT_THEME));
  });

  useEffect(() => {
    const root = document.documentElement;
    if (theme.mode === 'dark') {
      root.classList.add('dark');
    } else {
      root.classList.remove('dark');
    }
  }, [theme.mode]);

  const toggleMode = useCallback(() => {
    const newTheme: ThemeConfig = {
      ...theme,
      mode: (theme.mode === 'light' ? 'dark' : 'light') as ThemeMode,
    };
    setTheme(newTheme);
    save(KEYS.THEME, newTheme);
  }, [theme]);

  return { theme, toggleMode };
}
