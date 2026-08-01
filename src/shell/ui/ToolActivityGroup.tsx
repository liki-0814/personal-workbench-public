import { Fragment, useState, type ReactNode } from 'react';
import { ChevronRight } from 'lucide-react';
import { getToolActivitySummary, type ToolActivityItem } from './toolActivity';

interface Props {
  items: ToolActivityItem[];
  renderItem: (item: ToolActivityItem) => ReactNode;
  className?: string;
  summaryClassName?: string;
  listClassName?: string;
}

export default function ToolActivityGroup({
  items,
  renderItem,
  className = '',
  summaryClassName = '',
  listClassName = '',
}: Props) {
  const [open, setOpen] = useState(false);
  if (items.length === 0) return null;
  const summary = getToolActivitySummary(items);

  return (
    <details className={className} onToggle={event => setOpen(event.currentTarget.open)}>
      <summary className={summaryClassName}>
        <ChevronRight
          size={11}
          className={`flex-shrink-0 transition-transform ${open ? 'rotate-90' : ''}`}
          aria-hidden
        />
        <span title={summary.label}>{summary.label}</span>
        <small className={`is-${summary.status}`}>
          {summary.meta}{summary.kind ? ` · ${summary.kind}` : ''}
        </small>
      </summary>
      {open && (
        <div className={listClassName}>
          {items.map(item => <Fragment key={item.id}>{renderItem(item)}</Fragment>)}
        </div>
      )}
    </details>
  );
}
