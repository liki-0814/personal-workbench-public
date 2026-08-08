import { useEffect, useRef, type ButtonHTMLAttributes, type HTMLAttributes, type ReactNode } from 'react';

interface DropdownMenuProps extends Omit<HTMLAttributes<HTMLDivElement>, 'onKeyDown'> {
  label: string;
  onClose?: () => void;
  children: ReactNode;
}

export function DropdownMenu({ label, onClose, children, className = '', ...rest }: DropdownMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
  }, []);
  const focusItem = (direction: 1 | -1 | 'first' | 'last') => {
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? []);
    if (items.length === 0) return;
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = direction === 'first'
      ? items[0]
      : direction === 'last'
        ? items[items.length - 1]
        : items[(current + direction + items.length) % items.length];
    next.focus();
  };

  return (
    <div
      ref={menuRef}
      role="menu"
      aria-label={label}
      className={`precision-dropdown-menu ${className}`}
      onKeyDown={event => {
        if (event.key === 'ArrowDown') { event.preventDefault(); focusItem(1); }
        else if (event.key === 'ArrowUp') { event.preventDefault(); focusItem(-1); }
        else if (event.key === 'Home') { event.preventDefault(); focusItem('first'); }
        else if (event.key === 'End') { event.preventDefault(); focusItem('last'); }
        else if (event.key === 'Escape') { event.preventDefault(); onClose?.(); }
      }}
      {...rest}
    >
      {children}
    </div>
  );
}

interface DropdownMenuItemProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  icon?: ReactNode;
  danger?: boolean;
}

export function DropdownMenuItem({ icon, danger = false, className = '', children, type = 'button', ...rest }: DropdownMenuItemProps) {
  return (
    <button
      type={type}
      role="menuitem"
      className={`precision-dropdown-menu-item ${danger ? 'is-danger' : ''} ${className}`}
      {...rest}
    >
      {icon && <span className="precision-dropdown-menu-icon" aria-hidden>{icon}</span>}
      <span>{children}</span>
    </button>
  );
}

export function DropdownMenuSeparator() {
  return <div className="precision-dropdown-menu-separator" role="separator" />;
}

export function DropdownMenuHint({ children }: { children: ReactNode }) {
  return <small className="precision-dropdown-menu-hint">{children}</small>;
}
