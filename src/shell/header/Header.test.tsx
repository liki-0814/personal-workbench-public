import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Header from './Header';

vi.mock('@/core/utils', () => ({
  apiFetch: vi.fn(() => new Promise(() => undefined)),
}));

const theme = {
  mode: 'light' as const,
  fontSize: 'medium' as const,
  compactMode: false,
};

describe('Header primary navigation preference', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    sessionStorage.clear();
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.restoreAllMocks();
  });

  const renderHeader = async () => {
    await act(async () => root.render(
      <Header
        activeTab="workbench"
        onTabChange={vi.fn()}
        theme={theme}
        onToggleMode={vi.fn()}
        onOpenToolbox={vi.fn()}
      />,
    ));
  };

  it('defaults to a collapsed navigation when there is no saved preference', async () => {
    await renderHeader();

    expect(container.querySelector('.primary-rail')?.classList.contains('is-collapsed')).toBe(true);
    expect(sessionStorage.getItem('pwb_nav_collapsed')).toBe('1');
  });

  it('respects and persists an existing expanded preference', async () => {
    sessionStorage.setItem('pwb_nav_collapsed', '0');

    await renderHeader();

    expect(container.querySelector('.primary-rail')?.classList.contains('is-collapsed')).toBe(false);
    expect(sessionStorage.getItem('pwb_nav_collapsed')).toBe('0');
  });

  it('persists a user toggle for subsequent refreshes', async () => {
    await renderHeader();

    const toggle = container.querySelector('[title="展开导航"]') as HTMLButtonElement;
    await act(async () => toggle.click());

    expect(container.querySelector('.primary-rail')?.classList.contains('is-collapsed')).toBe(false);
    expect(sessionStorage.getItem('pwb_nav_collapsed')).toBe('0');
  });
});
