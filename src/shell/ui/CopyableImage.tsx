import { useState } from 'react';
import { Check, Copy, Download, ExternalLink, LoaderCircle } from 'lucide-react';
import { showToast } from '../state/toastStore';
import { copyImage, downloadImage } from './imageClipboard';

interface CopyableImageProps {
  src: string;
  alt?: string;
  compact?: boolean;
  className?: string;
}

export default function CopyableImage({ src, alt = '', compact = false, className = '' }: CopyableImageProps) {
  const [pending, setPending] = useState<'copy' | 'download' | null>(null);
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    setPending('copy');
    try {
      const outcome = await copyImage(src);
      setCopied(outcome === 'copied');
      if (outcome === 'copied') showToast({ type: 'success', message: '图片已复制到剪贴板' });
      if (outcome === 'downloaded_and_url_copied') showToast({ type: 'info', message: '剪贴板未授权，已下载 PNG 并复制图片地址' });
      if (outcome === 'downloaded') showToast({ type: 'info', message: '剪贴板未授权，已下载 PNG' });
      window.setTimeout(() => setCopied(false), 1800);
    } catch (error) {
      showToast({ type: 'error', message: error instanceof Error ? error.message : '复制图片失败' });
    } finally {
      setPending(null);
    }
  };

  const handleDownload = async () => {
    setPending('download');
    try {
      await downloadImage(src);
      showToast({ type: 'success', message: 'PNG 已下载' });
    } catch (error) {
      showToast({ type: 'error', message: error instanceof Error ? error.message : '下载图片失败' });
    } finally {
      setPending(null);
    }
  };

  return (
    <figure className={`copyable-image group relative m-0 overflow-hidden rounded-xl border border-black/10 bg-black/[0.025] dark:border-white/10 dark:bg-white/[0.035] ${compact ? 'max-w-[360px]' : 'max-w-[640px]'} ${className}`}>
      <img
        src={src}
        alt={alt}
        loading="lazy"
        className={`block h-auto w-auto max-w-full object-contain ${compact ? 'max-h-[300px]' : 'max-h-[560px]'}`}
      />
      <figcaption className="absolute bottom-2 right-2 flex items-center gap-1 rounded-lg border border-white/20 bg-neutral-950/75 p-1 text-white opacity-0 shadow-lg backdrop-blur-md transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
        <button type="button" onClick={() => window.open(src, '_blank', 'noopener,noreferrer')} className="rounded-md p-1.5 hover:bg-white/15 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1" aria-label="打开图片" title="打开图片">
          <ExternalLink size={15} />
        </button>
        <button type="button" onClick={handleCopy} disabled={pending !== null} className="rounded-md p-1.5 hover:bg-white/15 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 disabled:opacity-50" aria-label="复制图片" title="复制图片">
          {pending === 'copy' ? <LoaderCircle size={15} className="animate-spin" /> : copied ? <Check size={15} /> : <Copy size={15} />}
        </button>
        <button type="button" onClick={handleDownload} disabled={pending !== null} className="rounded-md p-1.5 hover:bg-white/15 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 disabled:opacity-50" aria-label="下载 PNG" title="下载 PNG">
          {pending === 'download' ? <LoaderCircle size={15} className="animate-spin" /> : <Download size={15} />}
        </button>
      </figcaption>
    </figure>
  );
}
