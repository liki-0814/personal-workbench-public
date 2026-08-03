import { useEffect, useState } from 'react';
import { STORAGE_SYNC_EVENT } from '@/core/storage/syncEngine';
import { getModels, type AiModelInfo } from './aiProviders';
import { getMoaConfig } from './moaStore';
import type { MoaConfig } from './moa';
import { getAppConfig, type AppConfig } from './appConfig';
import { getLocalConfig, type LocalConfig } from './localConfig';
import { useProviderStore } from './providerStore';

function useSyncedValue<T>(getter: () => T): T {
  const [value, setValue] = useState<T>(getter);
  useEffect(() => {
    const handler = () => setValue(getter());
    window.addEventListener(STORAGE_SYNC_EVENT, handler);
    return () => window.removeEventListener(STORAGE_SYNC_EVENT, handler);
  }, [getter]);
  return value;
}

export function useAiModels(): AiModelInfo[] {
  useProviderStore();
  return useSyncedValue(getModels);
}

export function useChatModels(): AiModelInfo[] {
  useProviderStore();
  return useSyncedValue(getModels);
}

export function useMoaConfig(): MoaConfig {
  return useSyncedValue(getMoaConfig);
}

export function useAppConfig(): AppConfig {
  return useSyncedValue(getAppConfig);
}

/** Snapshot of `~/.pwcli/config.json` (sensitive tokens masked, Harness MoA
 *  embedded, etc.). Refreshes via STORAGE_SYNC_EVENT when SSE pushes
 *  `local_config` or after a saveLocalConfig() round-trip. */
export function useLocalConfig(): LocalConfig {
  return useSyncedValue(getLocalConfig);
}
