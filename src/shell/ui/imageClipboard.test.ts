import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { copyImage, fetchImageBlob } from './imageClipboard';

vi.mock('@/core/config/backendUrl', () => ({ getBackendUrl: () => '' }));

const png = new Blob([new Uint8Array([137, 80, 78, 71])], { type: 'image/png' });

describe('imageClipboard', () => {
  const write = vi.fn();
  const writeText = vi.fn();

  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      blob: vi.fn().mockResolvedValue(png),
      json: vi.fn(),
    }));
    vi.stubGlobal('ClipboardItem', class ClipboardItem {
      constructor(public readonly items: Record<string, Blob>) {}
    });
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { write, writeText } });
    Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: vi.fn(() => 'blob:test') });
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: vi.fn() });
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined);
    write.mockReset().mockResolvedValue(undefined);
    writeText.mockReset().mockResolvedValue(undefined);
  });

  afterEach(() => vi.restoreAllMocks());

  it('copies a same-origin PNG directly', async () => {
    await expect(copyImage('/api/images/example.png')).resolves.toBe('copied');
    expect(fetch).toHaveBeenCalledWith('/api/images/example.png');
    expect(write).toHaveBeenCalledOnce();
  });

  it('uses the daemon for a remote image', async () => {
    await fetchImageBlob('https://cdn.example.com/figure.png');
    expect(fetch).toHaveBeenCalledWith('/api/images/fetch', expect.objectContaining({
      method: 'POST',
      body: JSON.stringify({ url: 'https://cdn.example.com/figure.png' }),
    }));
  });

  it('downloads PNG and copies the URL when image clipboard permission is denied', async () => {
    write.mockRejectedValueOnce(new DOMException('denied', 'NotAllowedError'));
    await expect(copyImage('https://cdn.example.com/figure.png')).resolves.toBe('downloaded_and_url_copied');
    expect(HTMLAnchorElement.prototype.click).toHaveBeenCalledOnce();
    expect(writeText).toHaveBeenCalledWith('https://cdn.example.com/figure.png');
  });
});

