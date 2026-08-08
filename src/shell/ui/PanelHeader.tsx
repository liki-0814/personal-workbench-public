import type { ReactNode } from 'react';

interface Props {
  title: string;
  actions?: ReactNode;
}

/** Standard workbench panel header: small title on the left, icon actions on the right. */
export default function PanelHeader({ title, actions }: Props) {
  return (
    <div className="flex items-center justify-between pb-3 mb-3 border-b border-gray-100 dark:border-[#1E2028]">
      <h3 className="text-[13px] font-semibold text-gray-900 dark:text-[#E5E7EB] tracking-[-0.01em]">{title}</h3>
      {actions && <div className="flex items-center gap-1">{actions}</div>}
    </div>
  );
}
