export function getBackendUrl(): string {
  return typeof __PWB_BACKEND_URL__ === 'string' ? __PWB_BACKEND_URL__ : '';
}
