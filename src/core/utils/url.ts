/** Candidate favicon URLs for a given page (root first, then current directory). */
export function getFaviconCandidates(url: string): string[] {
  try {
    const u = new URL(url);
    const origin = `${u.protocol}//${u.hostname}`;
    const candidates = [`${origin}/favicon.ico`];
    if (u.pathname && u.pathname !== '/' && !u.pathname.endsWith('.ico')) {
      const dir = u.pathname.replace(/\/[^/]*$/, '');
      if (dir && dir !== '/') {
        candidates.push(`${origin}${dir}/favicon.ico`);
      }
    }
    return candidates;
  } catch {
    return [];
  }
}

/** Back-compat: return the first favicon candidate. */
export function getFaviconUrl(url: string): string {
  return getFaviconCandidates(url)[0] || '';
}

/** Extract hostname from a URL. */
export function getDomainFromUrl(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}
