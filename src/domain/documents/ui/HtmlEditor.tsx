import { useRef, useState } from 'react';
import { ArrowDown, ArrowUp, Bold, Code2, Copy, Heading2, ImagePlus, Italic, List, MousePointer2, Plus, Trash2, Type } from 'lucide-react';
import type { EditableDocument, HtmlDocumentContent } from '../types';
import { documentAssetUrl, uploadDocumentAsset } from '../api';
import { prepareHtmlForFrame, serializeFrameDocument } from './htmlDocument';

interface Props {
  document: EditableDocument<HtmlDocumentContent>;
  onChange: (content: HtmlDocumentContent) => void;
}

export default function HtmlEditor({ document, onChange }: Props) {
  const [mode, setMode] = useState<'visual' | 'source'>('visual');
  const [source, setSource] = useState(document.content.html);
  const [frameHtml, setFrameHtml] = useState(() => prepareHtmlForFrame(document.content.html, document.manifest.id));
  const [uploading, setUploading] = useState(false);
  const [selectedLabel, setSelectedLabel] = useState('未选择区块');
  const [sourceError, setSourceError] = useState('');
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const selectionRef = useRef<Range | null>(null);
  const selectedRef = useRef<HTMLElement | null>(null);

  const emitFrameChange = () => {
    const frameDocument = iframeRef.current?.contentDocument;
    if (!frameDocument) return;
    const html = serializeFrameDocument(frameDocument, document.manifest.id);
    setSource(html);
    onChange({ html });
  };

  const enableEditing = () => {
    const frameDocument = iframeRef.current?.contentDocument;
    if (!frameDocument?.body) return;
    frameDocument.body.contentEditable = 'true';
    frameDocument.body.spellcheck = true;
    frameDocument.body.addEventListener('input', emitFrameChange);
    frameDocument.addEventListener('selectionchange', () => {
      const selection = frameDocument.getSelection();
      if (selection?.rangeCount) selectionRef.current = selection.getRangeAt(0).cloneRange();
    });
    frameDocument.addEventListener('click', event => {
      const target = event.target as HTMLElement | null;
      if (target?.closest('a')) event.preventDefault();
      const block = target?.closest<HTMLElement>('[data-block], section, article, figure, table, img, h1, h2, h3, p, ul, ol, blockquote');
      selectedRef.current?.removeAttribute('data-pwb-selected');
      selectedRef.current = block ?? null;
      if (block) {
        block.setAttribute('data-pwb-selected', 'true');
        const identity = block.dataset.block || block.id;
        setSelectedLabel(`${block.tagName.toLowerCase()}${identity ? ` · ${identity}` : ''}`);
      } else {
        setSelectedLabel('未选择区块');
      }
    });
  };

  const command = (name: string, value?: string) => {
    const frameDocument = iframeRef.current?.contentDocument;
    if (!frameDocument) return;
    frameDocument.execCommand(name, false, value);
    emitFrameChange();
    iframeRef.current?.contentWindow?.focus();
  };

  const switchMode = (next: 'visual' | 'source') => {
    if (next === mode) return;
    if (next === 'visual') {
      if (!commitSource()) return;
      setFrameHtml(prepareHtmlForFrame(source, document.manifest.id));
    }
    setMode(next);
  };

  const commitSource = () => {
    const lower = source.toLowerCase();
    if (!lower.includes('<html') || !lower.includes('<body')) {
      setSourceError('HTML 必须保留完整的 html 和 body 元素');
      return false;
    }
    setSourceError('');
    onChange({ html: source });
    return true;
  };

  const mutateBlock = (operation: 'add' | 'duplicate' | 'remove' | 'up' | 'down') => {
    const frameDocument = iframeRef.current?.contentDocument;
    if (!frameDocument?.body) return;
    const selected = selectedRef.current;
    if (operation === 'add') {
      const block = frameDocument.createElement('section');
      block.dataset.block = `block-${Date.now().toString(36)}`;
      block.innerHTML = '<h2>新区块</h2><p>点击这里开始编辑。</p>';
      if (selected?.parentElement) selected.insertAdjacentElement('afterend', block);
      else frameDocument.body.append(block);
      selectedRef.current = block;
      block.setAttribute('data-pwb-selected', 'true');
      setSelectedLabel(`section · ${block.dataset.block}`);
    } else if (selected && operation === 'duplicate') {
      const copy = selected.cloneNode(true) as HTMLElement;
      copy.removeAttribute('data-pwb-selected');
      if (copy.dataset.block) copy.dataset.block = `${copy.dataset.block}-copy-${Date.now().toString(36)}`;
      selected.insertAdjacentElement('afterend', copy);
    } else if (selected && operation === 'remove') {
      selected.remove();
      selectedRef.current = null;
      setSelectedLabel('未选择区块');
    } else if (selected && operation === 'up' && selected.previousElementSibling) {
      selected.parentElement?.insertBefore(selected, selected.previousElementSibling);
    } else if (selected && operation === 'down' && selected.nextElementSibling) {
      selected.nextElementSibling.insertAdjacentElement('afterend', selected);
    } else {
      return;
    }
    emitFrameChange();
  };

  const insertImage = async (file: File) => {
    setUploading(true);
    try {
      const asset = await uploadDocumentAsset(document.manifest.id, file);
      const frameDocument = iframeRef.current?.contentDocument;
      if (!frameDocument?.body) return;
      const image = frameDocument.createElement('img');
      image.src = documentAssetUrl(document.manifest.id, asset.path);
      image.alt = file.name.replace(/\.[^.]+$/, '');
      image.setAttribute('data-pwb-asset', asset.path);
      image.style.maxWidth = '100%';
      image.style.height = 'auto';
      const selection = frameDocument.getSelection();
      selection?.removeAllRanges();
      if (selectionRef.current) {
        selection?.addRange(selectionRef.current);
        selectionRef.current.insertNode(image);
      } else if (selectedRef.current) {
        selectedRef.current.insertAdjacentElement('afterend', image);
      } else {
        frameDocument.body.append(image);
      }
      emitFrameChange();
    } finally {
      setUploading(false);
    }
  };

  return (
    <div className="html-editor">
      <div className="html-editor-bar">
        <div className="html-editor-mode">
          <button type="button" className={mode === 'visual' ? 'is-active' : ''} onClick={() => switchMode('visual')}><MousePointer2 size={14} />可视编辑</button>
          <button type="button" className={mode === 'source' ? 'is-active' : ''} onClick={() => switchMode('source')}><Code2 size={14} />HTML</button>
        </div>
        {mode === 'visual' && (
          <div className="html-editor-format">
            <span className="html-editor-selection">{selectedLabel}</span>
            <button type="button" onClick={() => mutateBlock('add')} title="新增区块"><Plus size={14} /></button>
            <button type="button" disabled={!selectedRef.current} onClick={() => mutateBlock('duplicate')} title="复制区块"><Copy size={14} /></button>
            <button type="button" disabled={!selectedRef.current} onClick={() => mutateBlock('up')} title="上移区块"><ArrowUp size={14} /></button>
            <button type="button" disabled={!selectedRef.current} onClick={() => mutateBlock('down')} title="下移区块"><ArrowDown size={14} /></button>
            <button type="button" disabled={!selectedRef.current} onClick={() => mutateBlock('remove')} title="删除区块"><Trash2 size={14} /></button>
            <button type="button" onClick={() => command('formatBlock', 'p')} title="正文"><Type size={14} /></button>
            <button type="button" onClick={() => command('formatBlock', 'h2')} title="二级标题"><Heading2 size={14} /></button>
            <button type="button" onClick={() => command('bold')} title="粗体"><Bold size={14} /></button>
            <button type="button" onClick={() => command('italic')} title="斜体"><Italic size={14} /></button>
            <button type="button" onClick={() => command('insertUnorderedList')} title="列表"><List size={14} /></button>
            <button type="button" disabled={uploading} onClick={() => fileRef.current?.click()} title="插入图片"><ImagePlus size={14} /></button>
            <input ref={fileRef} hidden type="file" accept="image/png,image/jpeg,image/webp" onChange={event => {
              const file = event.target.files?.[0];
              if (file) void insertImage(file);
              event.target.value = '';
            }} />
          </div>
        )}
      </div>
      {mode === 'visual' ? (
        <iframe
          ref={iframeRef}
          className="html-editor-frame"
          sandbox="allow-same-origin"
          srcDoc={frameHtml}
          title={`编辑 ${document.manifest.title}`}
          onLoad={enableEditing}
        />
      ) : (
        <>
          {sourceError && <div className="html-source-error">{sourceError}</div>}
          <textarea
            className="html-source-editor"
            aria-label="HTML 源码"
            spellCheck={false}
            value={source}
            onChange={event => setSource(event.target.value)}
            onBlur={commitSource}
          />
        </>
      )}
    </div>
  );
}
