import { describe, expect, it } from 'vitest';
import { resolveInitialWorkspace } from './workspaceNavigation';

describe('resolveInitialWorkspace', () => {
  it('migrates the legacy toolbox tab into a workbench overlay', () => {
    expect(resolveInitialWorkspace('toolbox')).toEqual({
      activeTab: 'workbench',
      openToolbox: true,
    });
  });

  it('preserves chat and normalizes old or unknown tabs', () => {
    expect(resolveInitialWorkspace('chat')).toEqual({ activeTab: 'chat', openToolbox: false });
    expect(resolveInitialWorkspace('studio')).toEqual({ activeTab: 'chat', openToolbox: false });
    expect(resolveInitialWorkspace('todos')).toEqual({ activeTab: 'workbench', openToolbox: false });
    expect(resolveInitialWorkspace('unknown')).toEqual({ activeTab: 'workbench', openToolbox: false });
  });
});

