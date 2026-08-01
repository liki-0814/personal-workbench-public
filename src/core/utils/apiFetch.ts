import { getBackendUrl } from '@/core/config/backendUrl';

interface ApiFetchOptions extends RequestInit {
  /** Skip JSON parsing (for streaming responses) */
  raw?: boolean;
}

/**
 * Thin wrapper over fetch for internal API calls.
 * - Prepends backend URL
 * - Sets JSON content-type for string request bodies
 * - Parses JSON responses and preserves successful text responses (unless raw: true)
 * - Throws with descriptive error on non-ok responses
 */
export async function apiFetch<T = unknown>(path: string, options: ApiFetchOptions = {}): Promise<T> {
  const { raw, ...fetchOpts } = options;
  const url = getBackendUrl() + path;

  const headers = new Headers(fetchOpts.headers);
  if (typeof fetchOpts.body === 'string' && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  const response = await fetch(url, { ...fetchOpts, headers });

  if (!response.ok) {
    const text = await response.text().catch(() => '');
    let message = text;
    try {
      const parsed = JSON.parse(text) as { error?: unknown };
      if (typeof parsed.error === 'string') message = parsed.error;
    } catch {
      // Keep a non-JSON response body as the best available error detail.
    }
    throw new Error(message || `请求失败（${response.status}）`);
  }

  if (raw) return response as unknown as T;
  const body = await response.text();
  if (!body.trim()) return undefined as T;
  try {
    return JSON.parse(body) as T;
  } catch (error) {
    const contentType = response.headers.get('Content-Type')?.toLowerCase() ?? '';
    if (contentType.includes('json')) throw error;
    return body as T;
  }
}
