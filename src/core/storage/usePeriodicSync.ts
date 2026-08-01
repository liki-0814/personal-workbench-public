import { useEffect } from 'react';
import { STORAGE_SYNC_REQUEST_EVENT, syncFromServer, syncKeysFromServer } from './syncEngine';
import { getBackendUrl } from '@/core/config/backendUrl';
import { reloadLocalConfigFromSse } from '@/core/config/localConfig';

const SYNC_INTERVAL_MS = 60_000; // 60s fallback (SSE handles real-time)
const SSE_RECONNECT_BASE_MS = 1000;
const SSE_RECONNECT_MAX_MS = 30_000;

/**
 * Real-time sync via SSE + periodic fallback.
 * - Connects to /api/data/events for instant push notifications
 * - Falls back to 60s polling if SSE is unavailable
 * - Syncs on tab visibility change
 */
export function usePeriodicSync() {
  useEffect(() => {
    let inFlight: Promise<boolean> | null = null;
    const runSync = (force = false, reason?: string) => {
      if (inFlight && !force) return inFlight;
      const start = () => syncFromServer({ reason });
      inFlight = (inFlight && force ? inFlight.finally(start) : start()).finally(() => {
        inFlight = null;
      });
      return inFlight;
    };

    // --- SSE: real-time server push ---
    let eventSource: EventSource | null = null;
    let reconnectDelay = SSE_RECONNECT_BASE_MS;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let closed = false;
    let sseDebounce: ReturnType<typeof setTimeout> | null = null;
    let ssePendingKeys: string[] = [];
    let hadPriorConnection = false;

    function connectSSE() {
      if (closed) return;
      // Connect directly to backend for SSE (Vite proxy buffers streaming responses)
      const backendUrl = getBackendUrl();
      const url = backendUrl ? `${backendUrl}/api/data/events` : '/api/data/events';
      eventSource = new EventSource(url);

      eventSource.onopen = () => {
        // On reconnect after a prior connection, do full sync to catch missed events
        if (hadPriorConnection) {
          runSync(true, 'sse-reconnect').catch(() => {});
          reloadLocalConfigFromSse().catch(() => {});
        }
        hadPriorConnection = true;
        reconnectDelay = SSE_RECONNECT_BASE_MS; // reset on success
      };

      eventSource.onmessage = (event) => {
        // Parse SSE data to extract changed keys for incremental sync
        let keys: string[] | null = null;
        try {
          const parsed = JSON.parse(event.data);
          if (parsed && Array.isArray(parsed.keys)) {
            keys = parsed.keys;
          }
        } catch {
          // Not JSON — fall back to full sync (backward compatibility)
        }

        if (keys && keys.length > 0) {
          // Special pseudo-key: ~/.pwcli/config.json changed (Track H3). It's
          // NOT in KEYS so the regular incremental sync would silently skip it
          // — route to its dedicated REST client instead.
          if (keys.includes('local_config')) {
            reloadLocalConfigFromSse().catch(() => { /* ignore */ });
          }
          const dataKeys = keys.filter(k => k !== 'local_config');
          if (dataKeys.length === 0) return;
          // Accumulate keys during debounce window for batching
          for (const k of dataKeys) {
            if (!ssePendingKeys.includes(k)) ssePendingKeys.push(k);
          }
          if (sseDebounce) clearTimeout(sseDebounce);
          sseDebounce = setTimeout(() => {
            const batch = ssePendingKeys.splice(0);
            syncKeysFromServer(batch, { reason: 'sse' }).catch(() => {});
          }, 200);
        } else {
          // No parseable keys — full sync fallback
          if (sseDebounce) clearTimeout(sseDebounce);
          ssePendingKeys = [];
          sseDebounce = setTimeout(() => {
            runSync(true, 'sse').catch(() => {});
          }, 200);
        }
      };

      eventSource.onerror = () => {
        eventSource?.close();
        eventSource = null;
        // Reconnect with exponential backoff
        if (!closed) {
          reconnectTimer = setTimeout(connectSSE, reconnectDelay);
          reconnectDelay = Math.min(reconnectDelay * 2, SSE_RECONNECT_MAX_MS);
        }
      };
    }

    connectSSE();

    // --- Fallback: periodic pull (in case SSE is down) ---
    const intervalId = setInterval(() => {
      runSync(false, 'interval').catch(() => {});
      reloadLocalConfigFromSse().catch(() => {});
    }, SYNC_INTERVAL_MS);

    // --- Sync when tab becomes visible ---
    const handleVisibility = () => {
      if (document.visibilityState === 'visible') {
        runSync(false, 'visible').catch(() => {});
        reloadLocalConfigFromSse().catch(() => {});
        // Reconnect SSE if it was closed
        if (!eventSource && !reconnectTimer) connectSSE();
      }
    };
    document.addEventListener('visibilitychange', handleVisibility);

    // --- Explicit sync requests (e.g. after agent writes) ---
    const handleSyncRequest = (event: Event) => {
      const detail = (event as CustomEvent<{ reason?: string }>).detail;
      runSync(true, detail?.reason || 'request').catch(() => {});
    };
    window.addEventListener(STORAGE_SYNC_REQUEST_EVENT, handleSyncRequest);

    return () => {
      closed = true;
      clearInterval(intervalId);
      eventSource?.close();
      if (reconnectTimer) clearTimeout(reconnectTimer);
      if (sseDebounce) clearTimeout(sseDebounce);
      document.removeEventListener('visibilitychange', handleVisibility);
      window.removeEventListener(STORAGE_SYNC_REQUEST_EVENT, handleSyncRequest);
    };
  }, []);
}
