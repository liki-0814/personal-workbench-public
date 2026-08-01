import { load, save, KEYS } from '@/core/storage';

export function getMermaidEnabled(): boolean {
  return load(KEYS.MERMAID_ENABLED, true);
}

export function setMermaidEnabled(enabled: boolean) {
  save(KEYS.MERMAID_ENABLED, enabled);
}
