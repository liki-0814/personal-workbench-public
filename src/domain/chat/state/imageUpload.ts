/**
 * Upload base64 images to the backend filesystem, returning persistent URLs.
 * This prevents localStorage quota exhaustion from inline base64 data.
 * Images are resized before upload to stay within LLM API size limits.
 */

import { apiFetch } from '@/core/utils';

const MAX_DIMENSION = 1568;
const JPEG_QUALITY = 0.85;
const MAX_BYTES = 4 * 1024 * 1024; // 4MB per image after base64

/** Returns true if the string is a base64 data URL (not already a server URL). */
export function isBase64DataUrl(s: string): boolean {
  return s.startsWith('data:');
}

/**
 * Resize a base64 image to fit within MAX_DIMENSION, re-encode as JPEG if needed.
 * PNGs with transparency stay PNG; large photos become JPEG for size savings.
 */
function resizeImage(dataUrl: string): Promise<string> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      const { width, height } = img;
      if (width <= MAX_DIMENSION && height <= MAX_DIMENSION) {
        // Already small enough — check total size
        if (dataUrl.length <= MAX_BYTES * 1.37) {
          resolve(dataUrl);
          return;
        }
      }

      const scale = Math.min(1, MAX_DIMENSION / Math.max(width, height));
      const w = Math.round(width * scale);
      const h = Math.round(height * scale);

      const canvas = document.createElement('canvas');
      canvas.width = w;
      canvas.height = h;
      const ctx = canvas.getContext('2d')!;
      ctx.drawImage(img, 0, 0, w, h);

      // Try JPEG first (smaller), fall back to PNG for transparency
      let result = canvas.toDataURL('image/jpeg', JPEG_QUALITY);
      if (result.length > MAX_BYTES * 1.37) {
        result = canvas.toDataURL('image/jpeg', 0.6);
      }
      resolve(result);
    };
    img.onerror = () => resolve(dataUrl);
    img.src = dataUrl;
  });
}

/**
 * Upload a single base64 image to /api/images/upload.
 * Returns the server URL (e.g. /api/images/<uuid>.png).
 * On failure, returns the original base64 string (graceful degradation).
 */
export async function uploadImage(base64: string): Promise<string> {
  if (!isBase64DataUrl(base64)) return base64;
  try {
    const resized = await resizeImage(base64);
    const json = await apiFetch<{ url?: string }>('/api/images/upload', {
      method: 'POST',
      body: JSON.stringify({ data: resized }),
    });
    return json.url || resized;
  } catch (err) {
    console.warn('[imageUpload] Upload error:', err);
    return base64;
  }
}

/**
 * Upload multiple images in parallel.
 * Returns an array of URLs (same order as input).
 */
export async function uploadImages(images: string[]): Promise<string[]> {
  return Promise.all(images.map(img => uploadImage(img)));
}
