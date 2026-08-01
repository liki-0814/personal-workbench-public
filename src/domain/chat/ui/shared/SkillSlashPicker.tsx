import { useEffect, useMemo, useState, useRef } from 'react';
import { Zap, BookOpen, Archive } from 'lucide-react';
import type { AgentSkill } from '../../state/agentStore';

interface Props {
  skills: AgentSkill[];
  /** 当前输入文本，需以 / 开头才触发 */
  query: string;
  /** 选中后回调：返回软触发的指示文本（让 AI 调用该 skill）*/
  onPick: (directive: string, skill: AgentSkill) => void;
  onCommand?: (command: string) => void;
  onCancel: () => void;
}

export default function SkillSlashPicker({ skills, query, onPick, onCommand, onCancel }: Props) {
  const filterText = query.startsWith('/') ? query.slice(1).trim().toLowerCase() : '';
  const [activeIdx, setActiveIdx] = useState(0);
  const activeRef = useRef<HTMLButtonElement>(null);

  const filtered = useMemo(() => {
    const items = [...skills];
    const sorted = items.sort((a, b) =>
      Number(b.has_command) - Number(a.has_command) ||
      a.name.localeCompare(b.name, 'zh-CN'),
    );
    if (!filterText) return sorted;
    return sorted
      .filter(s =>
        s.name.toLowerCase().includes(filterText) ||
        s.description.toLowerCase().includes(filterText),
      );
  }, [skills, filterText]);
  const showCompact = !filterText || 'compact'.includes(filterText) || '压缩上下文'.includes(filterText);
  const itemCount = filtered.length + (showCompact ? 1 : 0);

  useEffect(() => { setActiveIdx(0); }, [filterText]);
  useEffect(() => { activeRef.current?.scrollIntoView({ block: 'nearest' }); }, [activeIdx]);

  // Soft-trigger directive: a clear instruction that makes the agent invoke the skill.
  const buildDirective = (s: AgentSkill) => `使用 ${s.name} skill：`;

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const handlesKey = e.key === 'Escape'
        || e.key === 'ArrowDown'
        || e.key === 'ArrowUp'
        || (e.key === 'Enter' && !e.shiftKey);
      if (!handlesKey) return;
      // This listener runs in capture phase. Stop the same Enter from reaching
      // the textarea after selection re-renders and accidentally submitting it.
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape') { onCancel(); }
      else if (e.key === 'ArrowDown') { setActiveIdx(i => Math.min(itemCount - 1, i + 1)); }
      else if (e.key === 'ArrowUp') { setActiveIdx(i => Math.max(0, i - 1)); }
      else if (e.key === 'Enter' && itemCount > 0) {
        if (showCompact && activeIdx === 0) {
          onCommand?.('/compact');
          return;
        }
        const s = filtered[activeIdx - (showCompact ? 1 : 0)];
        onPick(buildDirective(s), s);
      }
    };
    window.addEventListener('keydown', handler, true);
    return () => window.removeEventListener('keydown', handler, true);
  }, [filtered, showCompact, itemCount, activeIdx, onPick, onCommand, onCancel]);

  return (
    <div className="precision-popover precision-command-picker model-selector-dropdown p-2 max-h-[320px] overflow-y-auto" onMouseDown={e => e.preventDefault()}>
      {itemCount === 0 ? (
        <div className="precision-empty-copy p-4 text-center text-xs">
          无匹配 Skill{filterText ? `「${filterText}」` : ''}
        </div>
      ) : (
        <>
          <div className="precision-picker-label text-[10px] uppercase tracking-[0.16em] px-2 py-1 font-semibold flex items-center gap-1.5">
            <Zap size={10} /> Agent Skills {skills.length} · ↑↓ 选择 · Enter 调用
          </div>
          <div className="space-y-0.5">
            {showCompact && (
              <button
                ref={activeIdx === 0 ? activeRef : undefined}
                onClick={() => onCommand?.('/compact')}
                onMouseEnter={() => setActiveIdx(0)}
                className={`precision-picker-option w-full text-left px-3 py-2 flex items-start gap-2.5 ${activeIdx === 0 ? 'is-active' : ''}`}
              >
                <Archive size={13} className="precision-picker-icon flex-shrink-0 mt-0.5" />
                <div className="flex-1 min-w-0">
                  <div className="precision-option-title text-sm font-semibold">/compact</div>
                  <div className="precision-option-description text-[11px] mt-0.5">压缩当前模型上下文，完整聊天记录仍然保留</div>
                </div>
              </button>
            )}
            {filtered.map((s, i) => (
              <button
                key={s.name}
                ref={i + (showCompact ? 1 : 0) === activeIdx ? activeRef : undefined}
                onClick={() => onPick(buildDirective(s), s)}
                onMouseEnter={() => setActiveIdx(i + (showCompact ? 1 : 0))}
                className={`precision-picker-option w-full text-left px-3 py-2 flex items-start gap-2.5 ${
                  i + (showCompact ? 1 : 0) === activeIdx ? 'is-active' : ''
                }`}
              >
                {s.has_command
                    ? <Zap size={13} className="precision-picker-icon flex-shrink-0 mt-0.5" />
                  : <BookOpen size={13} className="precision-picker-icon flex-shrink-0 mt-0.5" />}
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="precision-option-title text-sm font-semibold truncate">{s.name}</span>
                    {s.has_command && (
                      <span className="precision-executable-badge text-[9px] px-1.5 py-0.5">
                        可执行
                      </span>
                    )}
                  </div>
                  <div className="precision-option-description text-[11px] mt-0.5 line-clamp-2">
                    {s.description.replace(/\s+/g, ' ').slice(0, 90)}
                  </div>
                </div>
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
