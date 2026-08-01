import { ChevronDown, FileCode2, Loader2 } from 'lucide-react';
import { parseUnifiedPatch } from '@/core/utils/unifiedDiff';

export interface DiffFileView {
  path: string;
  status: string;
  size: number;
  binary: boolean;
  truncated: boolean;
  patch?: string;
}

interface Props {
  files: DiffFileView[];
  expandedPath?: string;
  loadingPath?: string;
  onToggle: (path: string) => void;
}

export default function DiffViewer({ files, expandedPath, loadingPath, onToggle }: Props) {
  return <div className="review-diff-viewer">
    {files.map(file => {
      const expanded = expandedPath === file.path;
      return <section key={file.path} className="review-diff-file">
        <button type="button" onClick={() => onToggle(file.path)} aria-expanded={expanded}>
          {loadingPath === file.path ? <Loader2 size={12} className="animate-spin" /> : <FileCode2 size={12} />}
          <span>{file.path}</span>
          <small>{file.binary ? '二进制' : file.truncated ? '已截断' : file.status}</small>
          <ChevronDown size={12} className={expanded ? 'is-open' : ''} />
        </button>
        {expanded && <div className="review-diff-patch">
          {file.binary
            ? <p>二进制文件仅展示元数据，不读取内容。</p>
            : file.patch
              ? <pre>{parseUnifiedPatch(file.patch).map((line, index) => <code key={`${index}:${line.text}`} className={`is-${line.kind}`}>{line.text || ' '}{'\n'}</code>)}</pre>
              : <p>{file.truncated ? '文件过大，预览已截断。' : '没有可展示的文本差异。'}</p>}
        </div>}
      </section>;
    })}
  </div>;
}
