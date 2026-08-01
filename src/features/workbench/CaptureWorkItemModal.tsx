import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { AlertCircle, FolderOpen, Loader2, X } from 'lucide-react';
import {
  DirectoryPickerModal,
  type LocalProject,
  type ResolvedDirectory,
} from '@/domain/chat';
import { generateId } from '@/core/utils';
import { SelectField, useModalDialog } from '@/shell';
import type { WorkItemCaptureRequest, WorkItemExecutor } from './workItemCapture';

interface Props {
  open: boolean;
  projects: LocalProject[];
  onClose: () => void;
  onCapture: (request: WorkItemCaptureRequest) => Promise<void>;
}

const EXECUTOR_OPTIONS = [
  { value: 'auto', label: '自动选择' },
  { value: 'pwcli', label: 'Workbench Agent' },
  { value: 'codex', label: 'Codex' },
  { value: 'qoder', label: 'Qoder' },
  { value: 'kimi', label: 'Kimi' },
];

export default function CaptureWorkItemModal({ open, projects, onClose, onCapture }: Props) {
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const [prompt, setPrompt] = useState('');
  const [directory, setDirectory] = useState<ResolvedDirectory>();
  const [executor, setExecutor] = useState<WorkItemExecutor>('auto');
  const [requestId, setRequestId] = useState('');
  const [directoryPickerOpen, setDirectoryPickerOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState('');
  const dialogRef = useModalDialog({
    open,
    onClose: () => {
      if (!submitting) onClose();
    },
    dismissible: !submitting,
    initialFocusRef: promptRef,
  });

  useEffect(() => {
    if (!open) return;
    setPrompt('');
    setDirectory(undefined);
    setExecutor('auto');
    setRequestId(generateId());
    setDirectoryPickerOpen(false);
    setSubmitting(false);
    setError('');
  }, [open]);

  if (!open) return null;

  const submit = async () => {
    const trimmedPrompt = prompt.trim();
    if (!trimmedPrompt || !directory) return;
    setSubmitting(true);
    setError('');
    try {
      await onCapture({
        clientRequestId: requestId,
        prompt: trimmedPrompt,
        cwd: directory.canonicalPath,
        executorPreference: executor,
      });
      onClose();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '事项捕获失败，请重试');
    } finally {
      setSubmitting(false);
    }
  };

  return createPortal(
    <>
      <div
        className="capture-work-item-backdrop"
        onMouseDown={event => {
          if (!submitting && event.target === event.currentTarget) onClose();
        }}
      >
        <section
          ref={dialogRef}
          className="capture-work-item-dialog"
          role="dialog"
          aria-modal="true"
          aria-labelledby="capture-work-item-title"
          aria-describedby="capture-work-item-description"
          tabIndex={-1}
        >
          <header>
            <div>
              <small>快速入口</small>
              <h2 id="capture-work-item-title">捕获事项</h2>
              <p id="capture-work-item-description">写清完整目标，选择工作目录和执行器后立即开始。</p>
            </div>
            <button type="button" onClick={onClose} disabled={submitting} aria-label="关闭捕获事项">
              <X size={17} />
            </button>
          </header>

          <div className="capture-work-item-form">
            <label>
              <span>事项描述</span>
              <textarea
                ref={promptRef}
                value={prompt}
                onChange={event => setPrompt(event.target.value)}
                placeholder="例如：检查支付回调的重复入账问题，补充回归测试并给出变更说明"
                rows={6}
                disabled={submitting}
              />
            </label>

            <div className="capture-work-item-grid">
              <div className="capture-work-item-field">
                <span>本地目录</span>
                <button
                  type="button"
                  className="capture-work-item-directory"
                  onClick={() => setDirectoryPickerOpen(true)}
                  disabled={submitting}
                >
                  <FolderOpen size={15} />
                  <span title={directory?.displayPath || directory?.canonicalPath}>
                    {directory?.displayPath || directory?.canonicalPath || '选择工作文件夹'}
                  </span>
                  <small>{directory ? '更改' : '必选'}</small>
                </button>
              </div>

              <label className="capture-work-item-field">
                <span>执行器</span>
                <SelectField
                  value={executor}
                  options={EXECUTOR_OPTIONS}
                  onValueChange={value => setExecutor(value as WorkItemExecutor)}
                  disabled={submitting}
                  ariaLabel="选择事项执行器"
                  menuMinWidth={190}
                />
              </label>
            </div>

            {error && (
              <div className="capture-work-item-error" role="alert">
                <AlertCircle size={14} />
                {error}
              </div>
            )}
          </div>

          <footer>
            <button type="button" onClick={onClose} disabled={submitting}>取消</button>
            <button
              type="button"
              className="is-primary"
              disabled={submitting || !prompt.trim() || !directory}
              onClick={() => void submit()}
            >
              {submitting && <Loader2 size={14} className="animate-spin" />}
              捕获并开始
            </button>
          </footer>
        </section>
      </div>

      <DirectoryPickerModal
        open={directoryPickerOpen}
        projects={projects}
        initialPath={directory?.canonicalPath}
        onClose={() => setDirectoryPickerOpen(false)}
        onConfirm={resolved => {
          setDirectory(resolved);
          setDirectoryPickerOpen(false);
        }}
      />
    </>,
    document.body,
  );
}
