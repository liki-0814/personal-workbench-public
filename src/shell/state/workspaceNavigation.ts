import type { AppTab } from '@/types/app';

export interface InitialWorkspace {
  activeTab: AppTab;
  openToolbox: boolean;
}

export function resolveInitialWorkspace(saved: string | null): InitialWorkspace {
  if (saved === 'chat' || saved === 'studio') {
    return { activeTab: 'chat', openToolbox: false };
  }
  return { activeTab: 'workbench', openToolbox: saved === 'toolbox' };
}

