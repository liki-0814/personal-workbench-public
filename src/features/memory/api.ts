import { apiFetch } from '@/core/utils';

export interface MemoryIndexEntry {
  slug: string;
  summary: string;
  updated_at: number;
}

export interface MemoryStats {
  schema_version: number;
  active_entries: number;
  soft_deleted_entries: number;
  archived_entries: number;
  index_bytes: number;
  profile_bytes: number;
  index_lines: number;
  vector_indexed: number | null;
  has_profile: boolean;
}

export interface MemoryOverview {
  user_slug: string;
  profile: string;
  index_raw: string;
  entries: MemoryIndexEntry[];
  stats: MemoryStats;
}

export interface MemoryEntryDetail {
  slug: string;
  summary: string;
  content: string;
  created_at: number;
  updated_at: number;
  id?: string;
  supersedes?: string;
  deleted_at?: number | null;
}

export interface CreateEntryResponse {
  slug: string;
  requested_slug: string;
  entry: MemoryEntryDetail;
}

const BASE = '/api/agent/memory';

export function fetchMemoryOverview(): Promise<MemoryOverview> {
  return apiFetch<MemoryOverview>(`${BASE}/overview`);
}

export function updateMemoryProfile(content: string): Promise<{ content: string }> {
  return apiFetch(`${BASE}/profile`, {
    method: 'PUT',
    body: JSON.stringify({ content }),
  });
}

export function updateMemoryIndex(content: string): Promise<{ content: string }> {
  return apiFetch(`${BASE}/index`, {
    method: 'PUT',
    body: JSON.stringify({ content }),
  });
}

export function fetchMemoryEntry(slug: string): Promise<MemoryEntryDetail> {
  return apiFetch<MemoryEntryDetail>(`${BASE}/entries/${encodeURIComponent(slug)}`);
}

export function updateMemoryEntry(
  slug: string,
  body: { summary: string; content: string },
): Promise<MemoryEntryDetail> {
  return apiFetch<MemoryEntryDetail>(`${BASE}/entries/${encodeURIComponent(slug)}`, {
    method: 'PUT',
    body: JSON.stringify(body),
  });
}

export function createMemoryEntry(body: {
  slug: string;
  summary: string;
  content: string;
}): Promise<CreateEntryResponse> {
  return apiFetch<CreateEntryResponse>(`${BASE}/entries`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function deleteMemoryEntry(slug: string): Promise<{ deleted: string }> {
  return apiFetch(`${BASE}/entries/${encodeURIComponent(slug)}`, {
    method: 'DELETE',
  });
}

/** Matches pwcli `validate_slug`: 1–64 chars, [a-zA-Z0-9_-] */
export function isValidMemorySlug(slug: string): boolean {
  if (!slug || slug.length > 64) return false;
  return /^[a-zA-Z0-9_-]+$/.test(slug);
}
