import { useState, useEffect, useCallback, useRef } from 'react';
import { FileText } from 'lucide-react';
import { filterHidden, useShowHiddenFiles } from '@/core/utils/showHiddenFiles';
import { extractVariables, substitute, hasVariables } from '@/core/utils/variables';
import { listDirectory, readTextFile } from '../../api';
import '../studio.css';

interface FilePromptPickerProps {
  query: string;
  promptsDir: string;
  onPick: (content: string) => void;
  onClose: () => void;
}

interface PromptFile {
  name: string;
  title: string;
}

export default function FilePromptPicker({ query, promptsDir, onPick, onClose }: FilePromptPickerProps) {
  const showHidden = useShowHiddenFiles();
  const [files, setFiles] = useState<PromptFile[]>([]);
  const [activeIdx, setActiveIdx] = useState(0);
  const [variableMode, setVariableMode] = useState<{ content: string; vars: string[] } | null>(null);
  const [varValues, setVarValues] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);
  const activeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setFiles([]);
    setError(null);
  }, [promptsDir]);

  useEffect(() => {
    if (!promptsDir) return;
    const controller = new AbortController();
    setError(null);
    void listDirectory(promptsDir, controller.signal)
      .then(entries => {
        const mdFiles = entries
          .filter(e => e.type === 'file' && e.name.endsWith('.md'))
          .map(e => ({ name: e.name, title: e.name.replace(/\.md$/, '') }))
          .sort((a, b) => a.title.localeCompare(b.title));
        setFiles(mdFiles);
      })
      .catch(reason => {
        if (!controller.signal.aborted) {
          setError(reason instanceof Error ? reason.message : '无法读取指令目录');
        }
      });
    return () => controller.abort();
  }, [promptsDir, reloadToken]);

  const filterText = query.slice(1).toLowerCase();
  const filtered = filterHidden(files, showHidden).filter(f =>
    f.title.toLowerCase().includes(filterText)
  );

  useEffect(() => {
    setActiveIdx(0);
  }, [filterText]);

  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: 'nearest' });
  }, [activeIdx]);

  const pickFile = useCallback(async (file: PromptFile) => {
    try {
      setError(null);
      const data = await readTextFile(`${promptsDir}/${file.name}`);
      if (data.isBinary) throw new Error('指令文件不是文本文件');
      const content = data.content;
      if (hasVariables(content)) {
        setVariableMode({ content, vars: extractVariables(content) });
        setVarValues({});
      } else {
        onPick(content);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '读取指令文件失败');
    }
  }, [promptsDir, onPick]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (variableMode) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActiveIdx(i => Math.min(i + 1, filtered.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActiveIdx(i => Math.max(i - 1, 0));
    } else if (e.key === 'Enter' && filtered[activeIdx]) {
      e.preventDefault();
      pickFile(filtered[activeIdx]);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  }, [filtered, activeIdx, pickFile, onClose, variableMode]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onClose]);

  if (variableMode) {
    return (
      <div className="studio-prompt-popover p-3 max-h-[300px] overflow-y-auto">
        <div className="studio-prompt-label text-xs font-medium mb-2">填写变量</div>
        {variableMode.vars.map(v => (
          <div key={v} className="mb-2">
            <label className="studio-prompt-label text-xs block mb-0.5">{`{{${v}}}`}</label>
            <input
              className="studio-prompt-input w-full px-2 py-1 text-sm"
              value={varValues[v] || ''}
              onChange={e => setVarValues(prev => ({ ...prev, [v]: e.target.value }))}
              placeholder={v}
              autoFocus={v === variableMode.vars[0]}
            />
          </div>
        ))}
        <div className="flex gap-2 mt-2">
          <button
            className="studio-button studio-button-primary px-3 py-1 text-xs"
            onClick={() => {
              onPick(substitute(variableMode.content, varValues));
            }}
          >
            确认
          </button>
          <button
            className="studio-button px-3 py-1 text-xs"
            onClick={() => setVariableMode(null)}
          >
            返回
          </button>
        </div>
      </div>
    );
  }

  if (filtered.length === 0) {
    return (
      <div className="studio-prompt-popover p-3">
        <div className="studio-prompt-label text-xs text-center py-2">
          {error ? (
            <>
              <span className="block text-red-500">{error}</span>
              <button type="button" className="mt-2 underline" onClick={() => setReloadToken(value => value + 1)}>重试</button>
            </>
          ) : files.length === 0 ? `${promptsDir} 下无 .md 文件` : '无匹配指令'}
        </div>
      </div>
    );
  }

  return (
    <div
      className="studio-prompt-popover max-h-[240px] overflow-y-auto py-1"
      onKeyDown={handleKeyDown}
    >
      {error && <div className="px-3 py-1.5 text-xs text-red-500">{error}</div>}
      {filtered.slice(0, 8).map((file, idx) => (
        <div
          key={file.name}
          ref={idx === activeIdx ? activeRef : undefined}
          className={`studio-prompt-row flex items-center gap-2 px-3 py-1.5 cursor-pointer text-sm ${
            idx === activeIdx ? 'studio-prompt-row-active' : ''
          }`}
          onClick={() => pickFile(file)}
          onMouseEnter={() => setActiveIdx(idx)}
        >
          <FileText size={14} className="shrink-0 opacity-50" />
          <span className="truncate">{file.title}</span>
        </div>
      ))}
    </div>
  );
}
