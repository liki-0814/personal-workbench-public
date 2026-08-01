import { useEffect, useState } from 'react';
import { AlertTriangle, Loader2, Maximize2, Minimize2 } from 'lucide-react';
import MarkdownRenderer from '@/shell/ui/MarkdownRenderer';
import { readDocument } from '../api';
import type { DocumentRef, EditableDocument, MarkdownDocumentContent } from '../types';

interface Props {
  documentRef: DocumentRef;
  focused: boolean;
  onFocusChange: (focused: boolean) => void;
  onDirtyChange?: (dirty: boolean) => void;
}

export default function MarkdownDocumentViewer({
  documentRef,
  focused,
  onFocusChange,
  onDirtyChange,
}: Props) {
  const [document, setDocument] = useState<EditableDocument<MarkdownDocumentContent> | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    setDocument(null);
    setError('');
    onDirtyChange?.(false);
    void readDocument<MarkdownDocumentContent>(documentRef.id).then(value => {
      if (active) setDocument(value);
    }).catch(reason => {
      if (active) setError(reason instanceof Error ? reason.message : '无法打开 Markdown 文档');
    });
    return () => { active = false; };
  }, [documentRef.id, onDirtyChange]);

  if (error) {
    return <div className="document-workspace-state"><AlertTriangle size={20} /><strong>Markdown 文档无法打开</strong><p>{error}</p></div>;
  }
  if (!document) {
    return <div className="document-workspace-state"><Loader2 className="animate-spin" size={20} /><span>正在打开 Markdown 文档…</span></div>;
  }

  const origin = document.manifest.origin;
  const provenance = origin
    ? `${origin.agentName} · ${origin.executorId}`
    : '只读文档';

  return (
    <div className="document-workspace">
      <header className="document-toolbar">
        <div className="document-title-block">
          <span>{document.manifest.runtime === 'delegated' ? '协作者产出' : 'MARKDOWN'}</span>
          <strong>{document.manifest.title}</strong>
          <small>{provenance} · r{document.manifest.revision}</small>
        </div>
        <div className="document-toolbar-actions">
          <button type="button" onClick={() => onFocusChange(!focused)} title={focused ? '退出专注态' : '进入专注态'}>
            {focused ? <Minimize2 size={15} /> : <Maximize2 size={15} />}
          </button>
        </div>
      </header>
      <div className="markdown-document-scroll">
        <MarkdownRenderer content={document.content.markdown} className="document-markdown-body" />
      </div>
    </div>
  );
}
