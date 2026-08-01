import { X } from 'lucide-react';
import type { EditorTab } from '../../types';

interface FileTabsProps {
  tabs: EditorTab[];
  activeTabId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
}

export default function FileTabs({ tabs, activeTabId, onSelect, onClose }: FileTabsProps) {
  if (tabs.length === 0) return null;

  return (
    <div className="studio-file-tabs flex items-center overflow-x-auto shrink-0">
      {tabs.map(tab => (
        <div
          key={tab.id}
          className={`studio-file-tab group flex items-center gap-1.5 px-3 py-1.5 text-xs cursor-pointer transition-colors ${
            tab.id === activeTabId
              ? 'studio-file-tab-active'
              : 'studio-file-tab-idle'
          }`}
          onClick={() => onSelect(tab.id)}
        >
          {tab.dirty && <span className="studio-dirty-dot w-1.5 h-1.5 rounded-full bg-amber-400 shrink-0" />}
          <span className="truncate max-w-[150px]">{tab.name}</span>
          <button
            className="studio-tab-close opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity p-0.5"
            onClick={(e) => { e.stopPropagation(); onClose(tab.id); }}
          >
            <X size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}
