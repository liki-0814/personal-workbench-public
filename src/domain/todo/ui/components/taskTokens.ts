/** Shared design tokens for task rows and their metadata. */

import type { formatRelativeDate } from '../../utils/grouping';

export type RelativeTone = ReturnType<typeof formatRelativeDate>['tone'];

export const RELATIVE_TONE_COLOR: Record<RelativeTone, string> = {
  overdue: 'border-[#E4B7B7] bg-[#FBEFEF] text-[#A33D3D] dark:border-[#563236] dark:bg-[#2C2023] dark:text-[#F19A9A]',
  today:   'border-[#E4D0A4] bg-[#FAF5E8] text-[#8A641C] dark:border-[#55472C] dark:bg-[#28241C] dark:text-[#E8C779]',
  soon:    'border-[#BCC8F2] bg-[#EEF1FC] text-brand dark:border-[#37466F] dark:bg-[#202639] dark:text-[#9DB0FF]',
  later:   'border-[#D9DDE4] bg-[#F4F5F7] text-[#69717D] dark:border-[#363D47] dark:bg-[#1A1E25] dark:text-[#98A1AD]',
};
