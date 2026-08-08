import { getBackendUrl } from '@/core/config/backendUrl';

function getApiBase(): string {
  const backend = getBackendUrl();
  return backend ? `${backend}/api` : '/api';
}

async function request<T>(url: string, options?: RequestInit): Promise<T> {
  const response = await fetch(`${getApiBase()}${url}`, {
    headers: { 'Content-Type': 'application/json' },
    ...options,
  });
  if (!response.ok) {
    throw new Error(`API request failed: ${response.status} ${response.statusText}`);
  }
  return response.json();
}

interface ApiResponse<T> {
  success: boolean;
  data: T;
  error?: string;
}

export async function fetchAllData(): Promise<Record<string, string>> {
  const result = await request<ApiResponse<Record<string, string>>>('/data');
  if (!result.success) throw new Error(result.error || 'Failed to fetch all data');
  return result.data;
}

export async function fetchData<T>(key: string): Promise<T | null> {
  const result = await request<ApiResponse<T | null>>(`/data/${key}`);
  if (!result.success) throw new Error(result.error || `Failed to fetch key: ${key}`);
  return result.data;
}

export async function saveData<T>(key: string, value: T): Promise<void> {
  const result = await request<ApiResponse<void>>(`/data/${key}`, {
    method: 'PUT',
    body: JSON.stringify({ value }),
  });
  if (!result.success) throw new Error(result.error || `Failed to save key: ${key}`);
}

export async function batchSaveData(entries: Array<{ key: string; value: unknown }>): Promise<void> {
  const result = await request<ApiResponse<void>>('/data/batch', {
    method: 'POST',
    body: JSON.stringify({ entries }),
  });
  if (!result.success) throw new Error(result.error || 'Failed to batch save data');
}

export async function deleteData(key: string): Promise<void> {
  const result = await request<ApiResponse<void>>(`/data/${key}`, {
    method: 'DELETE',
  });
  if (!result.success) throw new Error(result.error || `Failed to delete key: ${key}`);
}
