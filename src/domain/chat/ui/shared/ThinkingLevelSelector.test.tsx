import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import ThinkingLevelSelector from './ThinkingLevelSelector';

describe('ThinkingLevelSelector', () => {
  let container: HTMLDivElement | null = null;
  let root: Root | null = null;

  afterEach(async () => {
    if (root) await act(async () => root?.unmount());
    document.querySelectorAll('.thinking-level-popover').forEach(node => node.remove());
    container?.remove();
    root = null;
    container = null;
  });

  const renderSelector = async (
    value: 'off' | 'low' | 'medium' | 'high' | 'xhigh' | 'max' = 'medium',
    onChange = vi.fn(),
    pending = false,
  ) => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    await act(async () => root?.render(
      <ThinkingLevelSelector
        value={value}
        levels={['off', 'low', 'medium', 'high', 'xhigh', 'max']}
        onChange={onChange}
        pending={pending}
      />,
    ));
    return onChange;
  };

  it('opens a slider containing only the supplied model levels', async () => {
    await renderSelector('high');
    const trigger = container!.querySelector('button')!;
    expect(trigger.textContent).toContain('思考：高');

    await act(async () => trigger.click());
    await act(async () => {});

    const range = document.querySelector<HTMLInputElement>('input[type="range"]')!;
    expect(range).not.toBeNull();
    expect(range.min).toBe('0');
    expect(range.max).toBe('5');
    expect(range.value).toBe('3');
    expect(range.getAttribute('aria-valuetext')).toBe('高');
    expect(document.querySelectorAll('.thinking-level-marker')).toHaveLength(6);
    expect(document.querySelector('.thinking-level-labels')?.textContent).toBe('关闭低中高超高最大');
  });

  it('changes the level through direct sliding and keyboard steps', async () => {
    const onChange = await renderSelector('medium');
    await act(async () => container!.querySelector('button')!.click());
    const range = document.querySelector<HTMLInputElement>('input[type="range"]')!;

    await act(async () => {
      const setRangeValue = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        'value',
      )?.set;
      setRangeValue?.call(range, '4');
      range.dispatchEvent(new Event('input', { bubbles: true }));
    });
    expect(onChange).toHaveBeenCalledWith('xhigh');

    await act(async () => range.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'ArrowRight',
      bubbles: true,
    })));
    expect(onChange).toHaveBeenLastCalledWith('high');

    await act(async () => range.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'End',
      bubbles: true,
    })));
    expect(onChange).toHaveBeenLastCalledWith('max');
  });

  it('closes with Escape and returns focus to the trigger', async () => {
    await renderSelector();
    const trigger = container!.querySelector<HTMLButtonElement>('button')!;
    await act(async () => trigger.click());
    const range = document.querySelector<HTMLInputElement>('input[type="range"]')!;

    await act(async () => range.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'Escape',
      bubbles: true,
    })));

    expect(document.querySelector('.thinking-level-popover')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it('communicates when a selected level is queued for the next model call', async () => {
    await renderSelector('high', vi.fn(), true);
    const trigger = container!.querySelector('button')!;
    expect(trigger.textContent).toContain('下一步：高');
    await act(async () => trigger.click());
    expect(document.querySelector('.thinking-level-guidance')?.textContent)
      .toContain('将在下一次模型调用生效');
  });
});
