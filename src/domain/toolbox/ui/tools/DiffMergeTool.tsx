import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowDownToLine,
  ArrowLeft,
  ArrowRight,
  ChevronDown,
  ChevronUp,
  Copy,
  Download,
  FileUp,
  PanelLeftClose,
  PanelsTopLeft,
  RotateCcw,
} from 'lucide-react';
import { showToast } from '@/shell';
import type { DiffMergeState } from '../../types';
import { buildMergeBlocks, changeBlocks, mergeFromChoices } from '../../utils/diffMerge';

interface Props {
  state: DiffMergeState;
  onChange: (state: DiffMergeState) => void;
}

type Side = 'original' | 'changed';

function downloadText(content: string) {
  const url = URL.createObjectURL(new Blob([content], { type: 'text/plain;charset=utf-8' }));
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = `merged-${new Date().toISOString().slice(0, 19).replace(/:/g, '-')}.txt`;
  anchor.click();
  URL.revokeObjectURL(url);
}

function countLines(value: string): number {
  return value.length === 0 ? 1 : value.split('\n').length;
}

function lineNumbers(count: number): string {
  return Array.from({ length: count }, (_, index) => String(index + 1)).join('\n');
}

function previewText(value: string): string {
  return value.length === 0 ? '（无内容）' : value.replace(/\n$/, '');
}

export default function DiffMergeTool({ state, onChange }: Props) {
  const initialStateRef = useRef(state);
  const draftRef = useRef(state);
  const onChangeRef = useRef(onChange);
  const persistTimerRef = useRef<number | null>(null);
  const originalInputRef = useRef<HTMLInputElement>(null);
  const changedInputRef = useRef<HTMLInputElement>(null);
  const originalEditorRef = useRef<HTMLTextAreaElement>(null);
  const changedEditorRef = useRef<HTMLTextAreaElement>(null);
  const originalGutterRef = useRef<HTMLPreElement>(null);
  const changedGutterRef = useRef<HTMLPreElement>(null);
  const mergeInputRef = useRef<HTMLTextAreaElement>(null);
  const syncingScrollRef = useRef(false);
  const [originalLineCount, setOriginalLineCount] = useState(() => countLines(state.original));
  const [changedLineCount, setChangedLineCount] = useState(() => countLines(state.changed));
  onChangeRef.current = onChange;

  const blocks = useMemo(
    () => buildMergeBlocks(state.original, state.changed, state.ignoreWhitespace),
    [state.original, state.changed, state.ignoreWhitespace],
  );
  const changes = useMemo(() => changeBlocks(blocks), [blocks]);
  const activeChange = changes.length === 0
    ? 0
    : Math.min(state.activeChange, changes.length - 1);
  const activeBlock = changes[activeChange];
  const originalLineNumbers = useMemo(() => lineNumbers(originalLineCount), [originalLineCount]);
  const changedLineNumbers = useMemo(() => lineNumbers(changedLineCount), [changedLineCount]);

  const emitImmediately = useCallback((next: DiffMergeState) => {
    if (persistTimerRef.current !== null) {
      window.clearTimeout(persistTimerRef.current);
      persistTimerRef.current = null;
    }
    draftRef.current = next;
    onChangeRef.current(next);
  }, []);

  const queueSnapshot = useCallback((next: DiffMergeState) => {
    draftRef.current = next;
    if (persistTimerRef.current !== null) window.clearTimeout(persistTimerRef.current);
    persistTimerRef.current = window.setTimeout(() => {
      persistTimerRef.current = null;
      onChangeRef.current(draftRef.current);
    }, 320);
  }, []);

  useEffect(() => {
    if (persistTimerRef.current !== null) return;
    draftRef.current = state;
    if (mergeInputRef.current && mergeInputRef.current.value !== state.merged) {
      mergeInputRef.current.value = state.merged;
    }
  }, [state]);

  useEffect(() => () => {
    if (persistTimerRef.current !== null) {
      window.clearTimeout(persistTimerRef.current);
      onChangeRef.current(draftRef.current);
    }
  }, []);

  const updateText = useCallback((side: Side, value: string) => {
    const current = draftRef.current;
    if (current[side] === value) return;
    const next = {
      ...current,
      [side]: value,
      merged: side === 'changed' ? value : current.merged,
      choices: {},
      activeChange: 0,
    };
    if (side === 'original') setOriginalLineCount(countLines(value));
    else {
      setChangedLineCount(countLines(value));
      if (mergeInputRef.current) mergeInputRef.current.value = value;
    }
    queueSnapshot(next);
  }, [queueSnapshot]);

  const setBothEditors = useCallback((original: string, changed: string) => {
    if (originalEditorRef.current) originalEditorRef.current.value = original;
    if (changedEditorRef.current) changedEditorRef.current.value = changed;
    setOriginalLineCount(countLines(original));
    setChangedLineCount(countLines(changed));
  }, []);

  const readFile = useCallback(async (side: Side, file?: File) => {
    if (!file) return;
    const value = await file.text();
    const current = draftRef.current;
    const next = {
      ...current,
      [side]: value,
      merged: side === 'changed' ? value : current.merged,
      choices: {},
      activeChange: 0,
    };
    const editor = side === 'original' ? originalEditorRef.current : changedEditorRef.current;
    if (editor) editor.value = value;
    if (side === 'original') setOriginalLineCount(countLines(value));
    else {
      setChangedLineCount(countLines(value));
      if (mergeInputRef.current) mergeInputRef.current.value = value;
    }
    emitImmediately(next);
  }, [emitImmediately]);

  const syncScroll = useCallback((side: Side, source: HTMLTextAreaElement) => {
    const gutter = side === 'original' ? originalGutterRef.current : changedGutterRef.current;
    if (gutter) gutter.scrollTop = source.scrollTop;
    if (draftRef.current.layout !== 'side-by-side' || syncingScrollRef.current) return;
    const target = side === 'original' ? changedEditorRef.current : originalEditorRef.current;
    if (!target) return;
    const sourceRange = Math.max(1, source.scrollHeight - source.clientHeight);
    const targetRange = Math.max(0, target.scrollHeight - target.clientHeight);
    syncingScrollRef.current = true;
    target.scrollTop = (source.scrollTop / sourceRange) * targetRange;
    const targetGutter = side === 'original' ? changedGutterRef.current : originalGutterRef.current;
    if (targetGutter) targetGutter.scrollTop = target.scrollTop;
    requestAnimationFrame(() => { syncingScrollRef.current = false; });
  }, []);

  const revealChange = useCallback((index: number) => {
    if (changes.length === 0) return;
    const normalized = (index + changes.length) % changes.length;
    const block = changes[normalized];
    if (originalEditorRef.current) {
      originalEditorRef.current.scrollTop = Math.max(0, (block.leftLine - 4) * 21);
      if (originalGutterRef.current) originalGutterRef.current.scrollTop = originalEditorRef.current.scrollTop;
    }
    if (changedEditorRef.current) {
      changedEditorRef.current.scrollTop = Math.max(0, (block.rightLine - 4) * 21);
      if (changedGutterRef.current) changedGutterRef.current.scrollTop = changedEditorRef.current.scrollTop;
    }
    emitImmediately({ ...draftRef.current, activeChange: normalized });
  }, [changes, emitImmediately]);

  const applyChoice = useCallback((id: string, choice: 'left' | 'right') => {
    const choices = { ...draftRef.current.choices, [id]: choice };
    const next = { ...draftRef.current, choices, merged: mergeFromChoices(blocks, choices) };
    if (mergeInputRef.current) mergeInputRef.current.value = next.merged;
    emitImmediately(next);
  }, [blocks, emitImmediately]);

  const applyAll = useCallback((choice: 'left' | 'right') => {
    const choices = choice === 'left'
      ? Object.fromEntries(changes.map(block => [block.id, 'left' as const]))
      : {};
    const next = {
      ...draftRef.current,
      choices,
      merged: choice === 'left' ? draftRef.current.original : draftRef.current.changed,
    };
    if (mergeInputRef.current) mergeInputRef.current.value = next.merged;
    emitImmediately(next);
  }, [changes, emitImmediately]);

  const copyMerged = useCallback(async () => {
    await navigator.clipboard.writeText(draftRef.current.merged);
    showToast({ message: '合并结果已复制', type: 'success' });
  }, []);

  return (
    <div className="diff-tool min-h-0 flex-1">
      <div className="tool-workbar diff-toolbar">
        <div className="tool-workbar-group">
          <button type="button" onClick={() => originalInputRef.current?.click()}>
            <FileUp size={14} />载入原文
          </button>
          <button type="button" onClick={() => changedInputRef.current?.click()}>
            <FileUp size={14} />载入新文
          </button>
          <input ref={originalInputRef} type="file" hidden onChange={event => readFile('original', event.target.files?.[0])} />
          <input ref={changedInputRef} type="file" hidden onChange={event => readFile('changed', event.target.files?.[0])} />
          <button
            type="button"
            onClick={() => {
              const current = draftRef.current;
              const next = {
                ...current,
                original: current.changed,
                changed: current.original,
                merged: current.original,
                choices: {},
                activeChange: 0,
              };
              setBothEditors(next.original, next.changed);
              if (mergeInputRef.current) mergeInputRef.current.value = next.merged;
              emitImmediately(next);
            }}
            disabled={!state.original && !state.changed}
          >
            <RotateCcw size={14} />交换
          </button>
        </div>

        <div className="diff-summary" aria-live="polite">
          <strong>{changes.length}</strong> 处差异
          <button type="button" onClick={() => revealChange(activeChange - 1)} disabled={changes.length === 0} aria-label="上一处差异">
            <ChevronUp size={14} />
          </button>
          <span>{changes.length === 0 ? '0 / 0' : `${activeChange + 1} / ${changes.length}`}</span>
          <button type="button" onClick={() => revealChange(activeChange + 1)} disabled={changes.length === 0} aria-label="下一处差异">
            <ChevronDown size={14} />
          </button>
        </div>

        <div className="tool-workbar-group tool-workbar-group-right">
          <label className="tool-check">
            <input
              type="checkbox"
              checked={state.ignoreWhitespace}
              onChange={event => emitImmediately({
                ...draftRef.current,
                ignoreWhitespace: event.target.checked,
                choices: {},
                activeChange: 0,
              })}
            />
            忽略空白
          </label>
          <button
            type="button"
            className={state.layout === 'side-by-side' ? 'is-active' : ''}
            onClick={() => emitImmediately({
              ...draftRef.current,
              layout: state.layout === 'side-by-side' ? 'inline' : 'side-by-side',
            })}
          >
            {state.layout === 'side-by-side' ? <PanelsTopLeft size={14} /> : <PanelLeftClose size={14} />}
            {state.layout === 'side-by-side' ? '并排' : '上下'}
          </button>
        </div>
      </div>

      <section className="diff-compare-section">
        <div className="diff-section-heading">
          <div>
            <span className="tool-eyebrow">Compare</span>
            <h2>文本比较</h2>
          </div>
          <p>两个编辑区保持同步滚动，停止输入后更新差异。</p>
        </div>

        <div className={`native-diff-grid ${state.layout === 'inline' ? 'is-stacked' : ''}`}>
          <div className="native-editor-pane">
            <div className="native-editor-heading">
              <strong>原始文本</strong><span>{originalLineCount} 行</span>
            </div>
            <div className="native-editor-body">
              <pre ref={originalGutterRef} className="native-editor-gutter" aria-hidden="true">{originalLineNumbers}</pre>
              <textarea
                ref={originalEditorRef}
                className="native-diff-input"
                defaultValue={initialStateRef.current.original}
                placeholder="在这里粘贴原始文本…"
                aria-label="原始文本"
                spellCheck={false}
                onChange={event => updateText('original', event.target.value)}
                onScroll={event => syncScroll('original', event.currentTarget)}
                onBlur={() => emitImmediately(draftRef.current)}
              />
            </div>
          </div>

          <div className="native-editor-pane">
            <div className="native-editor-heading">
              <strong>修改后文本</strong><span>{changedLineCount} 行</span>
            </div>
            <div className="native-editor-body">
              <pre ref={changedGutterRef} className="native-editor-gutter" aria-hidden="true">{changedLineNumbers}</pre>
              <textarea
                ref={changedEditorRef}
                className="native-diff-input"
                defaultValue={initialStateRef.current.changed}
                placeholder="在这里粘贴修改后的文本…"
                aria-label="修改后文本"
                spellCheck={false}
                onChange={event => updateText('changed', event.target.value)}
                onScroll={event => syncScroll('changed', event.currentTarget)}
                onBlur={() => emitImmediately(draftRef.current)}
              />
            </div>
          </div>
        </div>

        {activeBlock && (
          <div className="active-diff-card" aria-live="polite">
            <div className="active-diff-meta">
              <span>差异 {activeChange + 1}</span>
              <span>原文 L{activeBlock.leftLine} · 新文 L{activeBlock.rightLine}</span>
            </div>
            <div className="active-diff-preview">
              <pre className="is-removed">{previewText(activeBlock.left)}</pre>
              <pre className="is-added">{previewText(activeBlock.right)}</pre>
            </div>
            <div className="active-diff-actions">
              <button type="button" className={(state.choices[activeBlock.id] ?? 'right') === 'left' ? 'is-selected' : ''} onClick={() => applyChoice(activeBlock.id, 'left')}>
                <ArrowDownToLine size={13} />采用原文
              </button>
              <button type="button" className={(state.choices[activeBlock.id] ?? 'right') === 'right' ? 'is-selected' : ''} onClick={() => applyChoice(activeBlock.id, 'right')}>
                <ArrowDownToLine size={13} />采用新文
              </button>
            </div>
          </div>
        )}
      </section>

      <section className="merge-section">
        <div className="merge-heading">
          <div>
            <span className="tool-eyebrow">Merge result</span>
            <h3>合并结果</h3>
          </div>
          <div className="tool-workbar-group">
            <button type="button" onClick={() => applyAll('left')}><ArrowLeft size={14} />全部采用原文</button>
            <button type="button" onClick={() => applyAll('right')}><ArrowRight size={14} />全部采用新文</button>
            <button type="button" onClick={copyMerged} disabled={!state.merged}><Copy size={14} />复制</button>
            <button type="button" onClick={() => downloadText(draftRef.current.merged)} disabled={!state.merged}><Download size={14} />导出</button>
          </div>
        </div>

        <div className="merge-editor-shell">
          <textarea
            ref={mergeInputRef}
            className="merge-result-input"
            defaultValue={initialStateRef.current.merged}
            placeholder="合并结果会显示在这里，也可以直接编辑。"
            aria-label="合并结果"
            spellCheck={false}
            onChange={event => queueSnapshot({ ...draftRef.current, merged: event.target.value })}
            onBlur={() => emitImmediately(draftRef.current)}
          />
        </div>
      </section>
    </div>
  );
}
