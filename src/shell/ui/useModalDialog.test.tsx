import { act, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useModalDialog } from './useModalDialog';

function Harness({ dismissible = true, onClose }: { dismissible?: boolean; onClose: () => void }) {
  const [open, setOpen] = useState(true);
  const dialogRef = useModalDialog({
    open,
    dismissible,
    onClose: () => {
      onClose();
      setOpen(false);
    },
  });
  return open ? (
    <div ref={dialogRef} role="dialog" tabIndex={-1}>
      <button type="button">第一个</button>
      <button type="button">最后一个</button>
    </div>
  ) : null;
}

describe('useModalDialog', () => {
  let container: HTMLDivElement | null = null;

  afterEach(() => {
    container?.remove();
    container = null;
  });

  it('moves focus in, traps Tab, closes on Escape, and restores focus', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const trigger = document.createElement('button');
    document.body.append(trigger);
    trigger.focus();
    const onClose = vi.fn();
    const root = createRoot(container);

    await act(async () => root.render(<Harness onClose={onClose} />));
    await act(async () => new Promise(resolve => window.setTimeout(resolve, 0)));
    const buttons = Array.from(container.querySelectorAll('button'));
    expect(document.activeElement).toBe(buttons[0]);

    buttons[1].focus();
    await act(async () => document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true })));
    expect(document.activeElement).toBe(buttons[0]);

    await act(async () => document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
    expect(onClose).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(trigger);

    await act(async () => root.unmount());
    trigger.remove();
  });

  it('keeps a required modal open on Escape', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    const onClose = vi.fn();
    await act(async () => root.render(<Harness dismissible={false} onClose={onClose} />));
    await act(async () => document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });
});

