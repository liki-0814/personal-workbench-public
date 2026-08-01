import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, Bot, Download, Loader2, Maximize2, Minimize2, Redo2, Save, Undo2 } from 'lucide-react';
import type { DocumentRef, EditableDocument, HtmlDocumentContent } from '../types';
import { documentExportUrl, readDocument, updateDocument } from '../api';
import HtmlEditor from './HtmlEditor';
import MarkdownDocumentViewer from './MarkdownDocumentViewer';
import './documents.css';

interface Props {
  documentRef: DocumentRef;
  focused: boolean;
  onFocusChange: (focused: boolean) => void;
  onDirtyChange?: (dirty: boolean) => void;
  onAskAi?: (prompt: string) => void;
}

type SaveState = 'saved' | 'dirty' | 'saving' | 'conflict' | 'error';

export default function DocumentWorkspace(props: Props) {
  if (props.documentRef.kind === 'markdown') return <MarkdownDocumentViewer {...props} />;
  return <HtmlDocumentWorkspace {...props} />;
}

function HtmlDocumentWorkspace({ documentRef, focused, onFocusChange, onDirtyChange, onAskAi }: Props) {
  const [document, setDocument] = useState<EditableDocument<HtmlDocumentContent> | null>(null);
  const [saveState, setSaveState] = useState<SaveState>('saved');
  const [error, setError] = useState('');
  const [undoStack, setUndoStack] = useState<HtmlDocumentContent[]>([]);
  const [redoStack, setRedoStack] = useState<HtmlDocumentContent[]>([]);
  const [historyEpoch, setHistoryEpoch] = useState(0);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savingRef = useRef(false);
  const historyAtRef = useRef(0);
  const archifyFrameRef = useRef<HTMLIFrameElement | null>(null);
  const documentRefState = useRef<EditableDocument<HtmlDocumentContent> | null>(null);
  documentRefState.current = document;

  useEffect(() => {
    let active = true;
    setDocument(null);
    setError('');
    setUndoStack([]);
    setRedoStack([]);
    void readDocument<HtmlDocumentContent>(documentRef.id).then(value => {
      if (!active) return;
      setDocument(value);
      setSaveState('saved');
      onDirtyChange?.(false);
    }).catch(reason => {
      if (active) setError(reason instanceof Error ? reason.message : '无法打开 HTML 文档');
    });
    return () => { active = false; };
  }, [documentRef.id, onDirtyChange]);

  const save = useCallback(async () => {
    const current = documentRefState.current;
    if (!current || savingRef.current) return;
    savingRef.current = true;
    setSaveState('saving');
    setError('');
    try {
      const saved = await updateDocument(current);
      if (documentRefState.current === current) {
        setDocument(saved);
        setSaveState('saved');
        onDirtyChange?.(false);
      } else {
        setDocument(latest => latest ? { ...latest, manifest: saved.manifest } : saved);
        setSaveState('dirty');
      }
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : '保存失败';
      setError(message);
      setSaveState(message.includes('(409)') ? 'conflict' : 'error');
    } finally {
      savingRef.current = false;
      if (documentRefState.current !== current && documentRefState.current) {
        window.setTimeout(() => void save(), 0);
      }
    }
  }, [onDirtyChange]);

  useEffect(() => {
    if (saveState !== 'dirty') return;
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => void save(), 900);
    return () => { if (saveTimer.current) clearTimeout(saveTimer.current); };
  }, [save, saveState, document]);

  useEffect(() => {
    if (document?.manifest.runtime !== 'archify') return;
    const receiveArchifyDownload = (event: MessageEvent) => {
      if (event.source !== archifyFrameRef.current?.contentWindow) return;
      const message = event.data as { type?: unknown; filename?: unknown; blob?: unknown } | null;
      if (!message) return;
      const blob = message.blob;
      if (!(blob instanceof Blob)) return;
      if (message.type === 'pwb-archify-copy') {
        void (async () => {
          try {
            if (!navigator.clipboard?.write || typeof ClipboardItem === 'undefined') {
              throw new Error('当前浏览器不支持复制图片');
            }
            const mime = blob.type || 'image/png';
            await navigator.clipboard.write([new ClipboardItem({ [mime]: blob })]);
          } catch (reason) {
            window.alert(reason instanceof Error ? `复制失败：${reason.message}` : '复制失败');
          }
        })();
        return;
      }
      if (message.type !== 'pwb-archify-download' || typeof message.filename !== 'string') return;
      const url = URL.createObjectURL(blob);
      const link = window.document.createElement('a');
      link.href = url;
      link.download = message.filename;
      window.document.body.appendChild(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
    };
    window.addEventListener('message', receiveArchifyDownload);
    return () => window.removeEventListener('message', receiveArchifyDownload);
  }, [document?.manifest.id, document?.manifest.runtime]);

  const applyContent = (content: HtmlDocumentContent, recordHistory = true) => {
    setDocument(current => {
      if (!current) return current;
      if (recordHistory) {
        const now = Date.now();
        if (now - historyAtRef.current > 700) {
          setUndoStack(stack => [...stack.slice(-49), current.content]);
        }
        historyAtRef.current = now;
        setRedoStack([]);
      }
      return { ...current, manifest: { ...current.manifest, qaStatus: 'pending' }, content };
    });
    setSaveState('dirty');
    onDirtyChange?.(true);
  };

  const undo = () => {
    if (!document || undoStack.length === 0) return;
    const previous = undoStack[undoStack.length - 1];
    setUndoStack(stack => stack.slice(0, -1));
    setRedoStack(stack => [...stack, document.content]);
    setHistoryEpoch(epoch => epoch + 1);
    historyAtRef.current = 0;
    applyContent(previous, false);
  };

  const redo = () => {
    if (!document || redoStack.length === 0) return;
    const next = redoStack[redoStack.length - 1];
    setRedoStack(stack => stack.slice(0, -1));
    setUndoStack(stack => [...stack, document.content]);
    setHistoryEpoch(epoch => epoch + 1);
    historyAtRef.current = 0;
    applyContent(next, false);
  };

  if (error && !document) return <div className="document-workspace-state"><AlertTriangle size={20} /><strong>HTML 文档无法打开</strong><p>{error}</p></div>;
  if (!document) return <div className="document-workspace-state"><Loader2 className="animate-spin" size={20} /><span>正在打开 HTML 文档…</span></div>;

  if (document.manifest.runtime === 'archify') {
    return (
      <div className="document-workspace">
        <header className="document-toolbar">
          <div className="document-title-block">
            <span>ARCHIFY</span>
            <strong>{document.manifest.title}</strong>
            <small>原生交互图 · r{document.manifest.revision}</small>
          </div>
          <div className="document-toolbar-actions">
            <a href={documentExportUrl(document.manifest.id)} download><Download size={15} />导出 HTML</a>
            <button type="button" onClick={() => onFocusChange(!focused)} title={focused ? '退出专注态' : '进入专注态'}>{focused ? <Minimize2 size={15} /> : <Maximize2 size={15} />}</button>
          </div>
        </header>
        <iframe
          ref={archifyFrameRef}
          className="archify-document-frame"
          src={`/documents/${encodeURIComponent(document.manifest.id)}/preview`}
          sandbox="allow-scripts allow-downloads"
          allow="clipboard-write"
          title={`Archify 图表：${document.manifest.title}`}
        />
      </div>
    );
  }

  const statusLabel = saveState === 'saving' ? '正在保存…'
    : saveState === 'dirty' ? '有未保存修改'
      : saveState === 'conflict' ? '版本冲突'
        : saveState === 'error' ? '保存失败'
          : `已保存 · r${document.manifest.revision}`;

  return (
    <div className="document-workspace">
      <header className="document-toolbar">
        <div className="document-title-block">
          <span>HTML</span>
          <input value={document.manifest.title} aria-label="文档标题" onChange={event => {
            setDocument(current => current ? { ...current, manifest: { ...current.manifest, title: event.target.value } } : current);
            setSaveState('dirty');
            onDirtyChange?.(true);
          }} />
          <small className={`is-${saveState}`}>{statusLabel}</small>
        </div>
        <div className="document-toolbar-actions">
          <button type="button" disabled={!undoStack.length} onClick={undo} title="撤销"><Undo2 size={15} /></button>
          <button type="button" disabled={!redoStack.length} onClick={redo} title="重做"><Redo2 size={15} /></button>
          <i />
          <button type="button" onClick={() => onAskAi?.(`请修改 HTML 文档「${document.manifest.title}」（document_id=${document.manifest.id}）。我希望：`)}><Bot size={15} />AI 修改</button>
          <button type="button" disabled={saveState === 'saving'} onClick={() => void save()}><Save size={15} />保存</button>
          <a href={documentExportUrl(document.manifest.id)} download><Download size={15} />导出 HTML</a>
          <button type="button" onClick={() => onFocusChange(!focused)} title={focused ? '退出专注态' : '进入专注态'}>{focused ? <Minimize2 size={15} /> : <Maximize2 size={15} />}</button>
        </div>
      </header>
      {error && <div className="document-save-error"><AlertTriangle size={13} />{error}</div>}
      {document.manifest.qaStatus === 'warning' && <div className="document-qa-warning"><AlertTriangle size={13} />自动验收仍有警告，请检查视觉布局、资产和打印结果。</div>}
      <div className="document-editor-shell">
        <HtmlEditor
          key={historyEpoch}
          document={document}
          onChange={content => applyContent(content)}
        />
      </div>
    </div>
  );
}
