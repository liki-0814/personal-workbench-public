import { useEffect, useRef, useState } from 'react';
import { useTheme } from '@/shell/state/themeStore';
import { getMermaidEnabled } from '@/shell/state/mermaidSettings';
import {
  cacheGet,
  cacheKey,
  cacheSet,
  createMermaidRenderId,
  ensureMermaidInitialized,
  MERMAID_CONTAINER_CLASS,
  MERMAID_ERROR_CLASS,
} from './mermaidRenderer';

export function Mermaid({ code }: { code: string }) {
  const enabled = getMermaidEnabled();
  const { theme } = useTheme();
  const ref = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const key = cacheKey(theme.mode, code);

    const cached = cacheGet(key);
    if (cached && ref.current) {
      ref.current.innerHTML = cached;
      setError(null);
      return;
    }

    const timer = setTimeout(async () => {
      if (cancelled) return;
      try {
        const mermaid = await ensureMermaidInitialized(theme.mode);
        await mermaid.parse(code);
        const id = createMermaidRenderId();
        const { svg } = await mermaid.render(id, code);
        if (cancelled) return;
        cacheSet(key, svg);
        if (ref.current) {
          ref.current.innerHTML = svg;
          setError(null);
        }
      } catch (e) {
        if (!cancelled && !ref.current?.innerHTML) {
          setError(e instanceof Error ? e.message : String(e));
        }
      }
    }, 300);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [code, theme.mode, enabled]);

  if (!enabled) {
    return <pre className="markdown-pre"><code className="language-mermaid">{code}</code></pre>;
  }

  if (error) {
    return (
      <pre className={MERMAID_ERROR_CLASS}>
        Mermaid 渲染失败: {error}{'\n\n'}{code}
      </pre>
    );
  }
  return <div ref={ref} className={MERMAID_CONTAINER_CLASS} />;
}

