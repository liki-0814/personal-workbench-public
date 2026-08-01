import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowRight, ChevronLeft, ChevronRight, Pencil, X } from 'lucide-react';

export interface DecisionPromptOption {
  id: string;
  label: string;
  description?: string;
  recommended?: boolean;
}

interface Props {
  title: string;
  rationale?: string;
  options: DecisionPromptOption[];
  step?: number;
  total?: number;
  onSelect: (option: DecisionPromptOption) => Promise<void> | void;
  onCustom?: (answer: string) => Promise<void> | void;
  onSkip?: () => Promise<void> | void;
  onClose?: () => void;
}

export default function DecisionPrompt({ title, rationale, options, step = 1, total = 1, onSelect, onCustom, onSkip, onClose }: Props) {
  const [activeIndex, setActiveIndex] = useState(0);
  const [customOpen, setCustomOpen] = useState(false);
  const [custom, setCustom] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const customRef = useRef<HTMLInputElement>(null);

  const submit = useCallback(async (action: () => Promise<void> | void) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      await action();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '提交失败，请重试');
      setBusy(false);
    }
  }, [busy]);

  useEffect(() => {
    if (customOpen) customRef.current?.focus();
  }, [customOpen]);

  useEffect(() => {
    const handle = (event: KeyboardEvent) => {
      if (busy || customOpen || event.metaKey || event.ctrlKey || event.altKey) return;
      if (/^[1-9]$/.test(event.key)) {
        const index = Number(event.key) - 1;
        if (options[index]) {
          event.preventDefault();
          void submit(() => onSelect(options[index]));
        }
        return;
      }
      if (event.key === 'ArrowDown' || event.key === 'ArrowRight') {
        event.preventDefault();
        setActiveIndex(index => Math.min(options.length - 1, index + 1));
      } else if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') {
        event.preventDefault();
        setActiveIndex(index => Math.max(0, index - 1));
      } else if (event.key === 'Enter' && options[activeIndex]) {
        event.preventDefault();
        void submit(() => onSelect(options[activeIndex]));
      }
    };
    window.addEventListener('keydown', handle);
    return () => window.removeEventListener('keydown', handle);
  }, [activeIndex, busy, customOpen, onSelect, options, submit]);

  return (
    <section className="decision-prompt" aria-labelledby="decision-prompt-title">
      <header className="decision-prompt-header">
        <div>
          <h2 id="decision-prompt-title">{title}</h2>
          {rationale && <p>{rationale}</p>}
        </div>
        <nav aria-label="问题进度">
          <button type="button" disabled={step <= 1} aria-label="上一个问题"><ChevronLeft size={17} /></button>
          <span>{step} <small>of</small> {total}</span>
          <button type="button" disabled={step >= total} aria-label="下一个问题"><ChevronRight size={17} /></button>
          {onClose && <button type="button" onClick={onClose} aria-label="关闭问题"><X size={18} /></button>}
        </nav>
      </header>

      <div className="decision-prompt-options" role="listbox" aria-label={title}>
        {options.map((option, index) => (
          <button
            key={option.id}
            type="button"
            role="option"
            aria-selected={activeIndex === index}
            disabled={busy}
            className={activeIndex === index ? 'is-active' : ''}
            onMouseEnter={() => setActiveIndex(index)}
            onFocus={() => setActiveIndex(index)}
            onClick={() => void submit(() => onSelect(option))}
          >
            <span className="decision-prompt-number">{index + 1}</span>
            <span className="decision-prompt-copy">
              <strong>{option.label}</strong>
              {option.recommended && <em>推荐</em>}
              {option.description && <small>{option.description}</small>}
            </span>
            <ArrowRight className="decision-prompt-arrow" size={19} />
          </button>
        ))}
      </div>

      <footer className="decision-prompt-footer">
        {onCustom && (customOpen ? (
          <form onSubmit={event => {
            event.preventDefault();
            if (custom.trim()) void submit(() => onCustom(custom.trim()));
          }}>
            <Pencil size={15} />
            <input ref={customRef} value={custom} onChange={event => setCustom(event.target.value)} placeholder="告诉 AI 你希望怎样处理" />
            <button type="submit" disabled={!custom.trim() || busy}>发送</button>
          </form>
        ) : (
          <button type="button" className="decision-prompt-custom" onClick={() => setCustomOpen(true)}>
            <Pencil size={15} />自定义回答
          </button>
        ))}
        {onSkip && <button type="button" className="decision-prompt-skip" disabled={busy} onClick={() => void submit(onSkip)}>跳过</button>}
      </footer>
      {error && <p className="decision-prompt-error" role="alert">{error}</p>}
    </section>
  );
}
