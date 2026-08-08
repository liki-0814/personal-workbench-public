import type { ButtonHTMLAttributes, ReactNode } from 'react';

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** Used as both the tooltip and the aria-label. */
  title: string;
  children: ReactNode;
}

/** 7×7 ghost icon button used in panel headers and toolbars. */
export default function IconButton({ title, children, className = '', type = 'button', ...rest }: Props) {
  return (
    <button
      type={type}
      title={title}
      aria-label={title}
      className={`w-7 h-7 flex items-center justify-center rounded-[8px] text-gray-400 dark:text-[#4B5563] hover:text-gray-600 dark:hover:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] transition-colors disabled:opacity-40 ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}
