import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import DecisionPrompt from './DecisionPrompt';

describe('DecisionPrompt', () => {
  let container: HTMLDivElement | null = null;

  afterEach(() => {
    container?.remove();
    container = null;
  });

  it('renders progress and chooses numbered options from the keyboard', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    const onSelect = vi.fn();
    await act(async () => {
      root.render(<DecisionPrompt
        title="第一版导出范围怎么定？"
        step={3}
        total={3}
        options={[
          { id: 'pdf', label: '仅 PDF+源包', description: '优先稳定性', recommended: true },
          { id: 'pptx', label: '再加 PPTX', description: '增加实现量' },
        ]}
        onSelect={onSelect}
      />);
    });
    expect(container.textContent).toContain('3 of 3');
    expect(container.textContent).toContain('推荐');
    await act(async () => window.dispatchEvent(new KeyboardEvent('keydown', { key: '2' })));
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ id: 'pptx' }));
    await act(async () => root.unmount());
  });
});
