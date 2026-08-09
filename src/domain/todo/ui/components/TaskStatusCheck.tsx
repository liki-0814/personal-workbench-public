import { Check } from 'lucide-react';
import type { TodoItem } from '../../types';

interface Props {
  status?: TodoItem['status'];
  completed: boolean;
  onClick: (e: React.MouseEvent) => void;
}

/** 三态任务状态切换按钮（todo / in_progress / done）。 */
export default function TaskStatusCheck({ status, completed, onClick }: Props) {
  const cur = status || (completed ? 'done' : 'todo');
  if (cur === 'done') {
    return (
      <button
        type="button"
        onClick={onClick}
        className="flex h-[22px] w-[22px] flex-shrink-0 items-center justify-center rounded-[7px] border border-[#4F7A61] bg-[#4F7A61] text-white shadow-[inset_0_1px_0_rgba(255,255,255,0.2)] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50"
        title="已完成"
        aria-label="已完成，点击切换状态"
      >
        <Check size={11} strokeWidth={2.5} />
      </button>
    );
  }
  if (cur === 'in_progress') {
    return (
      <button
        type="button"
        onClick={onClick}
        className="relative flex h-[22px] w-[22px] flex-shrink-0 items-center justify-center rounded-[7px] border border-brand bg-[#EEF2FF] shadow-[inset_0_1px_0_rgba(255,255,255,0.65)] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 dark:border-[#7B96FF] dark:bg-[#7B96FF]/10"
        title="进行中"
        aria-label="进行中，点击切换状态"
      >
        <span className="h-2 w-2 rounded-[2px] bg-brand dark:bg-[#7B96FF]" />
      </button>
    );
  }
  return (
    <button
      type="button"
      onClick={onClick}
      className="h-[22px] w-[22px] flex-shrink-0 rounded-[7px] border-[1.5px] border-[#929BA8] bg-[#FAFBFC] shadow-[inset_0_1px_2px_rgba(31,41,55,0.08),0_1px_1px_rgba(31,41,55,0.04)] transition-[border-color,background-color,box-shadow] hover:border-brand hover:bg-white hover:shadow-[0_0_0_3px_rgba(53,89,214,0.10)] focus-visible:border-[#69717D] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#8F98A5]/50 dark:border-[#68717D] dark:bg-[#1A1E25] dark:shadow-[inset_0_1px_2px_rgba(0,0,0,0.28)] dark:hover:border-[#7B96FF]"
      title="已安排未进行"
      aria-label="已安排未进行，点击开始任务"
    />
  );
}
