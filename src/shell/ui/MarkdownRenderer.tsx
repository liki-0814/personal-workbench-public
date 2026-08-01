import { useState, useCallback, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import { Copy, Check } from 'lucide-react';
import { Mermaid } from './Mermaid';
import { highlightCode } from './codeHighlight';
import { getBackendUrl } from '@/core/config/backendUrl';
import CopyableImage from './CopyableImage';

interface Props {
  content: string;
  className?: string;
}

function extractText(node: unknown): string {
  if (typeof node === 'string') return node;
  if (typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(extractText).join('');
  if (node && typeof node === 'object') {
    const props = (node as { props?: { children?: unknown } }).props;
    if (props?.children !== undefined) return extractText(props.children);
  }
  return '';
}

function Pre({ code, language }: { code: string; language?: string }) {
  const [copied, setCopied] = useState(false);
  const highlighted = useMemo(() => highlightCode(code, language), [code, language]);

  const handleCopy = useCallback(() => {
    navigator.clipboard.writeText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  }, [code]);

  return (
    <pre className="markdown-pre relative group" data-language={highlighted.language}>
      <button
        onClick={handleCopy}
        className="absolute top-2 right-2 p-1.5 rounded-md bg-white/80 dark:bg-black/50 text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200 opacity-0 group-hover:opacity-100 transition-all"
        title="复制代码"
      >
        {copied ? <Check size={14} /> : <Copy size={14} />}
      </button>
      <code
        className={`hljs ${highlighted.language ? `language-${highlighted.language}` : ''}`}
        dangerouslySetInnerHTML={{ __html: highlighted.html }}
      />
    </pre>
  );
}

export default function MarkdownRenderer({ content, className = '' }: Props) {
  return (
    <div className={`markdown-body ${className}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[rehypeKatex]}
        components={{
          pre: ({ children }) => {
            const child = Array.isArray(children) ? children[0] : children;
            const childProps = (child as { props?: { className?: string; children?: React.ReactNode } } | null)?.props;
            const lang = /language-([^\s]+)/.exec(childProps?.className || '')?.[1];
            const code = extractText(childProps?.children).replace(/\n$/, '');
            if (lang === 'mermaid') {
              return <Mermaid code={code} />;
            }
            return <Pre code={code} language={lang} />;
          },
          img: ({ src, alt, ...props }) => {
            let proxiedSrc = src || '';
            if (proxiedSrc && !proxiedSrc.startsWith('data:') && !proxiedSrc.startsWith('http')) {
              let p = proxiedSrc;
              if (p.startsWith('file:///')) p = p.slice(7);
              else if (p.startsWith('file://')) p = p.slice(7);
              if (p.startsWith('/')) {
                const base = getBackendUrl() || '';
                proxiedSrc = `${base}/api/fs/read?path=${encodeURIComponent(p)}&raw=1`;
              }
            }
            return <CopyableImage src={proxiedSrc} alt={alt || ''} className={props.className || ''} />;
          },
          code: ({ className, children, ...props }) => {
            const isInline = !className;
            return isInline ? (
              <code className="markdown-inline-code" {...props}>
                {children}
              </code>
            ) : (
              <code className={className} {...props}>
                {children}
              </code>
            );
          },
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
