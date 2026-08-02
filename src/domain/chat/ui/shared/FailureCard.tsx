import type { FailureEnvelope, RecoveryAction } from '@/domain/chat/types';

export default function FailureCard({
  failure,
  recoveryPhase,
  compact = false,
  onAction,
}: {
  failure: FailureEnvelope;
  recoveryPhase?: string;
  compact?: boolean;
  onAction?: (action: RecoveryAction) => void;
}) {
  const recommended = failure.actions?.find(action => action.recommended) || failure.actions?.[0];
  const secondary = failure.actions?.find(action => action !== recommended);
  const recovering = recoveryPhase === 'auto_retrying' || failure.disposition === 'auto_retrying';
  return (
    <div className={`precision-failure-card ${compact ? 'is-compact' : ''}`} data-class={failure.class}>
      <div className="precision-failure-title">
        {recovering
          ? `正在恢复 ${Math.min((failure.attempt || 0) + 1, failure.maxAttempts || 2)}/${failure.maxAttempts || 2}`
          : failure.userMessage}
      </div>
      {!recovering && (
        <div className="precision-failure-meta">
          <span>{labelForClass(failure.class)}</span>
          {failure.impact && <span>{failure.impact}</span>}
        </div>
      )}
      {!recovering && (recommended || secondary) && (
        <div className="precision-failure-actions">
          {recommended && (
            <button
              type="button"
              className="is-primary"
              onClick={() => onAction?.(recommended)}
              disabled={!onAction}
            >
              {recommended.label}
            </button>
          )}
          {secondary && (
            <button
              type="button"
              onClick={() => onAction?.(secondary)}
              disabled={!onAction}
            >
              {secondary.label}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function labelForClass(value: string): string {
  switch (value) {
    case 'transient': return '临时故障';
    case 'permission_required': return '需要权限';
    case 'input_required': return '需要补充信息';
    case 'conflict': return '冲突';
    case 'interrupted': return '执行中断';
    case 'validation': return '参数无效';
    default: return '失败';
  }
}
