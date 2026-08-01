// Global type compatibility layer
// All domain types are re-exported from their respective domain modules.
// New code should import directly from @/domain/<domain> or @/shell.

// Todo
export type { TodoType, TodoSubTask, TodoStatus, RepeatType, TodoItem, Goal } from '@/domain/todo';

// Theme (shell)
export type { ThemeMode, ThemeConfig } from '@/shell/types';

// Pomodoro
export type { PomodoroRecord } from '@/domain/pomodoro';

// Toolbox
export type { ClipboardItem, ClipboardItemType, ToolboxState } from '@/domain/toolbox';


// Habit
export type { HabitItem, HabitFrequency, HabitRecord } from '@/domain/habit';

// App-level types
export * from './app';
