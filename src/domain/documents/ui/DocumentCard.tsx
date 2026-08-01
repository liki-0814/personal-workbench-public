import { ArrowRight, FileCode2, FileText, ShieldAlert } from 'lucide-react';
import type { DocumentRef } from '../types';
import './document-card.css';

export default function DocumentCard({ document, onOpen }: { document: DocumentRef; onOpen: () => void }) {
  const archify = document.runtime === 'archify';
  const markdown = document.kind === 'markdown';
  return (
    <button type="button" className="chat-document-card" onClick={onOpen}>
      <span className="chat-document-icon">{markdown ? <FileText size={18} /> : <FileCode2 size={18} />}</span>
      <span><small>{markdown ? (document.runtime === 'delegated' ? '协作者 Markdown 产出' : 'Markdown 文档') : archify ? 'Archify 交互架构图' : '可编辑 HTML 文档'}</small><strong>{document.title}</strong><em>revision {document.revision}{document.qaStatus === 'warning' ? ' · 有验收警告' : ''}</em></span>
      {document.qaStatus === 'warning' && <ShieldAlert className="chat-document-warning" size={15} />}
      <ArrowRight size={17} />
    </button>
  );
}
