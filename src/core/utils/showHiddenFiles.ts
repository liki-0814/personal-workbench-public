import { useCallback } from 'react';
import { getAppConfig, setAppConfig } from '@/core/config/appConfig';
import { useAppConfig } from '@/core/config/hooks';

/** 控制 Chat / Studio 文件浏览器是否显示 . 开头的文件/文件夹。
 *  统一从 SettingsModal「系统集成」配置（server.showHiddenFiles）读取。 */
export function useShowHiddenFiles(): boolean {
  const cfg = useAppConfig();
  return cfg.showHiddenFiles === 'true';
}

/** 不需要响应式订阅时的同步读取。 */
export function getShowHiddenFiles(): boolean {
  return getAppConfig().showHiddenFiles === 'true';
}

/** 写入：写回 app_config，并持久化到 config.json。 */
export function useSetShowHiddenFiles(): (next: boolean) => void {
  return useCallback((next: boolean) => {
    const current = getAppConfig();
    setAppConfig({ ...current, showHiddenFiles: next ? 'true' : 'false' });
  }, []);
}

export function filterHidden<T extends { name: string }>(entries: T[], showHidden: boolean): T[] {
  return showHidden ? entries : entries.filter(e => !e.name.startsWith('.'));
}
