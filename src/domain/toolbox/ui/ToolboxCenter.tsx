import { AnimatePresence, motion } from 'framer-motion';
import { Wrench, X } from 'lucide-react';
import { useModalDialog } from '@/shell';
import type { ClipboardItem, ClipboardItemType } from '../types';
import ClipboardPanel from './tab/ClipboardPanel';

interface Props {
  open: boolean;
  onClose: () => void;
  history: ClipboardItem[];
  onAdd: (content: string) => void;
  onRemove: (id: string) => void;
  onClear: () => void;
  onCopy: (content: string, type?: ClipboardItemType) => Promise<boolean>;
}

export default function ToolboxCenter({
  open,
  onClose,
  history,
  onAdd,
  onRemove,
  onClear,
  onCopy,
}: Props) {
  const dialogRef = useModalDialog({ open, onClose });

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="toolbox-center-overlay"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.2, ease: 'easeOut' }}
          onMouseDown={event => {
            if (event.target === event.currentTarget) onClose();
          }}
        >
          <motion.section
            ref={dialogRef}
            className="toolbox-center"
            role="dialog"
            aria-modal="true"
            aria-labelledby="toolbox-center-title"
            tabIndex={-1}
            initial={{ opacity: 0, y: 10, scale: 0.99 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 6, scale: 0.995 }}
            transition={{ duration: 0.2, ease: [0.23, 1, 0.32, 1] }}
          >
            <header className="toolbox-center-header">
              <div>
                <Wrench size={18} aria-hidden="true" />
                <span>
                  <strong id="toolbox-center-title">工具中心</strong>
                  <small>剪贴板、Diff 与时间转换</small>
                </span>
              </div>
              <button type="button" onClick={onClose} aria-label="关闭工具中心">
                <X size={18} />
              </button>
            </header>
            <div className="toolbox-center-body">
              <ClipboardPanel
                history={history}
                onAdd={onAdd}
                onRemove={onRemove}
                onClear={onClear}
                onCopy={onCopy}
              />
            </div>
          </motion.section>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

