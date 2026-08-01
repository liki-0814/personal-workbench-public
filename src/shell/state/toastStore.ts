import { useState, useCallback, useRef } from 'react';

export interface ToastItem {
  id: string;
  message: string;
  type: 'success' | 'error' | 'info';
  duration?: number;
  action?: {
    label: string;
    onClick: () => void;
  };
}

let globalAddToast: ((toast: Omit<ToastItem, 'id'>) => void) | null = null;

export function showToast(toast: Omit<ToastItem, 'id'>) {
  globalAddToast?.(toast);
}

export function useToast() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const timersRef = useRef<Map<string, number>>(new Map());

  const removeToast = useCallback((id: string) => {
    setToasts(prev => prev.filter(t => t.id !== id));
    const timer = timersRef.current.get(id);
    if (timer) {
      window.clearTimeout(timer);
      timersRef.current.delete(id);
    }
  }, []);

  const addToast = useCallback((toast: Omit<ToastItem, 'id'>) => {
    const id = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const item: ToastItem = { ...toast, id };
    setToasts(prev => [...prev, item]);

    const duration = toast.duration ?? 3000;
    const timer = window.setTimeout(() => {
      removeToast(id);
    }, duration);
    timersRef.current.set(id, timer);

    return id;
  }, [removeToast]);

  // Expose globally for non-React callers
  globalAddToast = addToast;

  return { toasts, addToast, removeToast };
}
