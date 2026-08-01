import { CheckCircle, AlertCircle, Info, X } from 'lucide-react';
import type { ToastItem } from '../state/toastStore';

interface Props {
  toasts: ToastItem[];
  onRemove: (id: string) => void;
}

const iconMap = {
  success: CheckCircle,
  error: AlertCircle,
  info: Info,
};

export default function ToastContainer({ toasts, onRemove }: Props) {
  if (toasts.length === 0) return null;

  return (
    <div className="toast-stack" aria-live="polite">
      {toasts.map(toast => {
        const Icon = iconMap[toast.type];
        return (
          <div
            key={toast.id}
            className={`toast-item toast-${toast.type}`}
          >
            <Icon size={15} className="toast-icon" />
            <span className="toast-message">{toast.message}</span>
            {toast.action && (
              <button
                onClick={() => {
                  toast.action!.onClick();
                  onRemove(toast.id);
                }}
                className="toast-action"
              >
                {toast.action.label}
              </button>
            )}
            <button
              onClick={() => onRemove(toast.id)}
              className={`toast-close ${toast.action ? '' : 'ml-auto'}`}
              aria-label="关闭通知"
            >
              <X size={12} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
