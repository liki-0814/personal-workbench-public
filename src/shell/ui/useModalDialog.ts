import { useEffect, useRef, type RefObject } from 'react';

const FOCUSABLE_SELECTOR = [
  'button:not(:disabled)',
  '[href]',
  'input:not(:disabled)',
  'select:not(:disabled)',
  'textarea:not(:disabled)',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

const dialogStack: symbol[] = [];

interface ModalDialogOptions {
  open: boolean;
  onClose: () => void;
  dismissible?: boolean;
  initialFocusRef?: RefObject<HTMLElement | null>;
  returnFocusRef?: RefObject<HTMLElement | null>;
}

export function useModalDialog<T extends HTMLElement = HTMLDivElement>({
  open,
  onClose,
  dismissible = true,
  initialFocusRef,
  returnFocusRef,
}: ModalDialogOptions) {
  const dialogRef = useRef<T>(null);
  const onCloseRef = useRef(onClose);
  const dismissibleRef = useRef(dismissible);
  onCloseRef.current = onClose;
  dismissibleRef.current = dismissible;

  useEffect(() => {
    if (!open) return;
    const token = Symbol('modal-dialog');
    const previousFocus = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const explicitReturnFocus = returnFocusRef?.current;
    dialogStack.push(token);

    const focusTimer = window.setTimeout(() => {
      const dialog = dialogRef.current;
      if (!dialog) return;
      const initial = initialFocusRef?.current
        ?? dialog.querySelector<HTMLElement>(FOCUSABLE_SELECTOR)
        ?? dialog;
      initial.focus();
    }, 0);

    const handleKeyDown = (event: KeyboardEvent) => {
      if (dialogStack[dialogStack.length - 1] !== token) return;
      const dialog = dialogRef.current;
      if (!dialog) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        if (dismissibleRef.current) onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
      if (focusable.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.clearTimeout(focusTimer);
      document.removeEventListener('keydown', handleKeyDown);
      const index = dialogStack.lastIndexOf(token);
      if (index >= 0) dialogStack.splice(index, 1);
      const returnTarget = explicitReturnFocus ?? previousFocus;
      if (returnTarget?.isConnected) returnTarget.focus();
    };
  }, [initialFocusRef, open, returnFocusRef]);

  return dialogRef;
}
