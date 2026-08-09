import { createPortal } from 'react-dom';
import { AnimatePresence, motion } from 'framer-motion';
import { useModalDialog } from './useModalDialog';

interface Props {
  open: boolean;
  title: string;
  description?: string;
  confirmLabel?: string;
  onConfirm: () => void;
  onClose: () => void;
}

/** 破坏性操作确认弹窗：初始焦点落在「取消」上，退出与进入对称。 */
export default function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = '删除',
  onConfirm,
  onClose,
}: Props) {
  const dialogRef = useModalDialog({ open, onClose });

  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18, ease: 'easeOut' }}
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 backdrop-blur-sm"
          onClick={onClose}
        >
          <motion.div
            ref={dialogRef}
            role="dialog"
            aria-modal="true"
            aria-label={title}
            initial={{ opacity: 0, scale: 0.96, y: 8 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.97, transition: { duration: 0.15, ease: 'easeOut' } }}
            transition={{ type: 'spring', stiffness: 320, damping: 26 }}
            className="w-full max-w-[340px] mx-4 rounded-2xl border border-[var(--border-default)] bg-[var(--surface-0)] p-5 shadow-2xl"
            onClick={e => e.stopPropagation()}
          >
            <h3 className="text-[14px] font-semibold text-[var(--text-primary)]">{title}</h3>
            {description && (
              <p className="mt-2 text-[12px] leading-5 text-[var(--text-secondary)]">{description}</p>
            )}
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className="h-8 cursor-pointer rounded-[8px] px-3 text-[12px] font-medium text-[var(--text-secondary)] transition-colors hover:bg-[var(--surface-2)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/25"
              >
                取消
              </button>
              <button
                type="button"
                onClick={onConfirm}
                className="h-8 cursor-pointer rounded-[8px] bg-[#EF4444] px-3 text-[12px] font-medium text-white transition-colors hover:bg-[#DC2626] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-500/30"
              >
                {confirmLabel}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}
