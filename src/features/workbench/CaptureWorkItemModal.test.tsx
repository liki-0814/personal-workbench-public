import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import CaptureWorkItemModal from './CaptureWorkItemModal';

vi.mock('@/domain/chat', () => ({
  DirectoryPickerModal: ({
    open,
    onConfirm,
  }: {
    open: boolean;
    onConfirm: (directory: {
      canonicalPath: string;
      displayPath: string;
      fsBase: string;
      readable: boolean;
    }) => void;
  }) => open ? (
    <button
      type="button"
      onClick={() => onConfirm({
        canonicalPath: '/workspace/project',
        displayPath: '~/project',
        fsBase: '/workspace',
        readable: true,
      })}
    >
      确认测试目录
    </button>
  ) : null,
}));

describe('CaptureWorkItemModal', () => {
  let container: HTMLDivElement | null = null;
  let root: Root | null = null;

  afterEach(async () => {
    if (root) await act(async () => root?.unmount());
    container?.remove();
    document.body.querySelectorAll('.pwb-select-menu').forEach(menu => menu.remove());
    container = null;
    root = null;
  });

  const renderModal = async (onCapture = vi.fn().mockResolvedValue(undefined)) => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    const onClose = vi.fn();
    await act(async () => root?.render(
      <CaptureWorkItemModal
        open
        projects={[]}
        onClose={onClose}
        onCapture={onCapture}
      />,
    ));
    return { onCapture, onClose };
  };

  it('does not create data when cancelled', async () => {
    const { onCapture, onClose } = await renderModal();
    const cancel = Array.from(document.body.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.textContent === '取消');

    await act(async () => cancel?.click());

    expect(onClose).toHaveBeenCalledOnce();
    expect(onCapture).not.toHaveBeenCalled();
  });

  it('submits the full prompt, confirmed directory, and selected executor', async () => {
    const { onCapture } = await renderModal();
    const textarea = document.body.querySelector<HTMLTextAreaElement>('.capture-work-item-form textarea');
    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set;
    await act(async () => {
      setValue?.call(textarea, '  检查发布流程并补齐验证  ');
      textarea?.dispatchEvent(new Event('input', { bubbles: true }));
    });

    const chooseDirectory = Array.from(document.body.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.textContent?.includes('选择工作文件夹'));
    await act(async () => chooseDirectory?.click());
    const confirmDirectory = Array.from(document.body.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.textContent === '确认测试目录');
    await act(async () => confirmDirectory?.click());

    const executor = document.body.querySelector<HTMLButtonElement>('[aria-label="选择事项执行器"]');
    await act(async () => executor?.click());
    const qoder = Array.from(document.body.querySelectorAll<HTMLButtonElement>('[role="option"]'))
      .find(option => option.textContent?.includes('Qoder'));
    await act(async () => qoder?.click());

    const submit = Array.from(document.body.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.textContent?.includes('捕获并开始'));
    await act(async () => {
      submit?.click();
      await Promise.resolve();
    });

    expect(onCapture).toHaveBeenCalledWith({
      clientRequestId: expect.any(String),
      prompt: '检查发布流程并补齐验证',
      cwd: '/workspace/project',
      executorPreference: 'qoder',
    });
  });
});
