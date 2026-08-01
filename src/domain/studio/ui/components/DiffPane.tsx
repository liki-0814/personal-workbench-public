import { FileDiff, Loader2 } from 'lucide-react';
import { parseUnifiedPatch } from '@/core/utils/unifiedDiff';

interface Props {
  patch?: string;
  loading: boolean;
  error?: string;
  binary?: boolean;
  truncated?: boolean;
}

export default function DiffPane({ patch, loading, error, binary, truncated }: Props) {
  const lines = patch ? parseUnifiedPatch(patch) : [];
  if (loading) return <div className="studio-diff-state"><Loader2 size={16} className="animate-spin" />读取冻结 diff…</div>;
  if (error) return <div className="studio-diff-state is-error">{error}</div>;
  if (binary) return <div className="studio-diff-state"><FileDiff size={18} />二进制文件不展示文本 diff。</div>;
  if (lines.length === 0) return <div className="studio-diff-state">该 revision 没有可展示的文本差异。</div>;
  return (
    <div className="studio-diff-scroll">
      {truncated && <div className="studio-diff-warning">diff 过长，当前仅展示冻结预览。</div>}
      <pre>{lines.map((line, index) => (
        <code key={`${index}:${line.text}`} className={`is-${line.kind}`}>
          <span>{line.oldLine ?? ''}</span><span>{line.newLine ?? ''}</span>
          <em>{line.kind === 'add' ? '+' : line.kind === 'delete' ? '−' : ' '}</em>
          <b>{line.text.replace(/^[+-]/, '') || ' '}</b>{'\n'}
        </code>
      ))}</pre>
    </div>
  );
}
