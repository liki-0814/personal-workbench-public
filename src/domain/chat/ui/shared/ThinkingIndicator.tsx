interface Props {
  /** Optional label override; defaults to "回复中". */
  label?: string;
  /** Compact = smaller dots, no gap padding. */
  compact?: boolean;
}

/**
 * Pre-stream waiting indicator shown after the model accepts a request
 * and before the first visible token arrives.
 *
 * Animations live in src/index.css (.thinking-dots / .thinking-label).
 *
 * Note: label is intentionally "回复中" not "思考中" — the latter clashes
 * with the deep-thinking toggle's active-state copy and confuses users
 * into thinking the toggle was auto-enabled.
 */
export function ThinkingIndicator({ label = '回复中', compact = false }: Props) {
  return (
    <div className={`inline-flex items-center ${compact ? 'gap-1.5' : 'gap-2'}`}>
      <span className="thinking-dots" aria-hidden>
        <span /><span /><span />
      </span>
      <span className={`thinking-label font-medium tracking-wide ${compact ? 'text-[11px]' : 'text-xs'}`}>
        {label}
      </span>
    </div>
  );
}
