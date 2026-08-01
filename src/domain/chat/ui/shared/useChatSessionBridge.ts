import { useEffect, useRef } from 'react';
import { getModels } from '@/core/config';
import type { ChatSession, ChatMessage, AiModel } from '../../types';

interface AiBinding {
  model: AiModel;
  setModel: (m: AiModel) => void;
  sendMessage: (text: string, images?: string[]) => void;
}

interface UseChatSessionBridgeArgs {
  activeSession: ChatSession | null;
  activeSessionId: string | null;
  ai: AiBinding;
  onUpdateSession: (id: string, patch: Partial<ChatSession>) => void;
  onPendingMessage?: (text: string, images: string[] | undefined, collaboration: boolean) => void;
}

/**
 * Bridges activeSession <-> ai (the useAiChat instance):
 * - On session switch: pull session.model into ai.model.
 * - On user-driven ai.model change: push back to session.
 *   (Suppresses the immediate push-back triggered by the pull, which would otherwise
 *   cause a write of the old model into the new session before re-render settles.)
 * - Defers a queued first-message until after session id updates (avoids losing it
 *   to React state batching). Caller writes into pendingFirstMessageRef.
 *
 * Returns pendingFirstMessageRef and a stable aiRef for callers that want to
 * trigger sendMessage from elsewhere without re-running effects on every ai change.
 */
export function useChatSessionBridge({
  activeSession,
  activeSessionId,
  ai,
  onUpdateSession,
  onPendingMessage,
}: UseChatSessionBridgeArgs) {
  const aiRef = useRef(ai);
  aiRef.current = ai;

  const pendingFirstMessageRef = useRef<{ text: string; images?: string[]; collaboration?: boolean } | null>(null);
  const pendingSenderRef = useRef(onPendingMessage);
  pendingSenderRef.current = onPendingMessage;

  // suppressNextPushRef: the session-id-change effect bumps this so the
  // model-change effect (firing in the same tick) skips the write-back.
  const suppressNextPushRef = useRef(false);

  // Pull from session on switch
  useEffect(() => {
    if (activeSession) {
      const modelName =
        getModels().find(
          m => m.id === activeSession.model || m.name === activeSession.model
        )?.id || activeSession.model;
      // 研究模式不再强制切模型——pwcli 直接用 chat 选的模型
      if (aiRef.current.model !== modelName) {
        suppressNextPushRef.current = true;
        aiRef.current.setModel(modelName);
      }
    }
    // Only on session id change. ai.setModel is stable; including ai would re-fire constantly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSession?.id]);

  // Push ai.model back into session
  useEffect(() => {
    if (suppressNextPushRef.current) {
      suppressNextPushRef.current = false;
      return;
    }
    if (activeSessionId && activeSession && ai.model !== activeSession.model) {
      onUpdateSession(activeSessionId, { model: ai.model });
    }
  }, [ai.model, activeSessionId, activeSession, onUpdateSession]);

  // Pending first-message: fire after session id flips. Use aiRef so cleanup
  // doesn't depend on the changing ai object.
  useEffect(() => {
    if (activeSessionId && pendingFirstMessageRef.current) {
      const pending = pendingFirstMessageRef.current;
      pendingFirstMessageRef.current = null;
      const timer = setTimeout(() => {
        if (pendingSenderRef.current) {
          pendingSenderRef.current(pending.text, pending.images, Boolean(pending.collaboration));
        } else {
          aiRef.current.sendMessage(pending.text, pending.images);
        }
      }, 50);
      return () => clearTimeout(timer);
    }
  }, [activeSessionId]);

  return {
    aiRef,
    pendingFirstMessageRef,
  };
}

export type { ChatMessage };
