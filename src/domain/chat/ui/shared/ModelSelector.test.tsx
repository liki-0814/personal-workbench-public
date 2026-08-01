import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import ModelSelector from './ModelSelector';

vi.mock('@/core/config/hooks', () => ({
  useChatModels: () => [
    { id: 'gpt-5', name: 'GPT 5', providerName: 'OpenAI' },
    { id: 'claude-4', name: 'Claude 4', providerName: 'Anthropic' },
  ],
}));

describe('ModelSelector', () => {
  let container: HTMLDivElement | null = null;

  afterEach(() => {
    document.querySelectorAll('.pwb-select-menu').forEach(node => node.remove());
    container?.remove();
    container = null;
  });

  it('resolves legacy labels and uses shared listbox keyboard behavior', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    const onChange = vi.fn();
    await act(async () => root.render(<ModelSelector value="GPT 5" onChange={onChange} />));

    const trigger = container.querySelector('button') as HTMLButtonElement;
    expect(trigger.textContent).toContain('GPT 5');
    await act(async () => trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })));
    await act(async () => {});

    const listbox = document.querySelector('[role="listbox"]') as HTMLElement;
    const options = Array.from(document.querySelectorAll<HTMLButtonElement>('[role="option"]'));
    expect(listbox).toBeTruthy();
    expect(options).toHaveLength(2);
    expect(options[0].getAttribute('aria-selected')).toBe('true');

    await act(async () => new Promise<void>(resolve => window.requestAnimationFrame(() => resolve())));
    options[0].focus();
    await act(async () => listbox.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })));
    expect(document.activeElement).toBe(options[1]);
    await act(async () => options[1].click());
    expect(onChange).toHaveBeenCalledWith('claude-4');
    expect(document.querySelector('[role="listbox"]')).toBeNull();
    await act(async () => root.unmount());
  });
});
