import { Code2, ListTodo, Globe, BookOpen, Lightbulb, ArrowUpRight } from 'lucide-react';
import type { ReactNode } from 'react';

interface PromptCard {
  text: string;
  hint?: string;
  icon?: ReactNode;
}

interface Props {
  prompts: (string | PromptCard)[];
  onPick: (prompt: string) => void;
  variant?: 'panel' | 'tab';
  title?: string;
  subtitle?: string;
}

/** Default icons follow the prompt's task category without decorative color coding. */
const DEFAULT_ICONS = [
  <Code2 size={16} />,
  <ListTodo size={16} />,
  <Globe size={16} />,
  <Lightbulb size={16} />,
  <BookOpen size={16} />,
];

function normalizePrompt(p: string | PromptCard, idx: number): Required<PromptCard> {
  const defaultIcon = DEFAULT_ICONS[idx % DEFAULT_ICONS.length];
  if (typeof p === 'string') {
    return { text: p, hint: '', icon: defaultIcon };
  }
  return {
    text: p.text,
    hint: p.hint || '',
    icon: p.icon ?? defaultIcon,
  };
}

export default function EmptyChatHero({
  prompts,
  onPick,
  variant = 'tab',
  title = '有什么可以帮你的？',
  subtitle,
}: Props) {
  const isPanel = variant === 'panel';
  const cards = prompts.map(normalizePrompt);

  if (isPanel) {
    // Compact version for the workbench panel — small icon, tight chips.
    return (
      <div className="h-full flex flex-col items-center justify-center text-center py-6">
        <div className="precision-agent-mark precision-agent-mark-sm mb-3" aria-hidden>AI</div>
        <p className="precision-panel-title text-sm font-medium mb-1">{title}</p>
        {subtitle && <p className="precision-empty-copy text-xs mb-3">{subtitle}</p>}
        <div className="flex flex-wrap gap-1.5 justify-center px-2">
          {cards.map((c) => (
            <button
              key={c.text}
              onClick={() => onPick(c.text)}
              className="precision-prompt-chip px-3 py-1.5 text-xs"
            >
              {c.text}
            </button>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col items-center justify-center px-6">
      <div className="w-full max-w-[820px]">
        {/* Hero */}
        <div className="precision-hero mb-10">
          <div className="precision-hero-kicker">
            <span className="precision-agent-mark precision-agent-mark-sm" aria-hidden>AI</span>
            <span>工作台 · 新会话</span>
          </div>
          <h1 className="pwb-hero-title mt-4 mb-2">{title}</h1>
          {subtitle && <p className="pwb-hero-sub">{subtitle}</p>}
        </div>

        {/* Prompt grid */}
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          {cards.map((c) => (
            <button
              key={c.text}
              onClick={() => onPick(c.text)}
              className="pwb-prompt-card group"
            >
              <div className="flex items-start gap-3">
                <span className="pwb-prompt-card-icon flex-shrink-0">
                  {c.icon}
                </span>
                <div className="flex-1 min-w-0">
                  <div className="pwb-prompt-card-title">{c.text}</div>
                  {c.hint && <div className="pwb-prompt-card-hint mt-0.5">{c.hint}</div>}
                </div>
                <ArrowUpRight size={14} className="precision-prompt-arrow" aria-hidden />
              </div>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
