export type TodoType = 'today' | 'week' | 'longterm';

export interface TodoSubTask {
  id: string;
  title: string;
  completed: boolean;
  dueDate?: string;
}

export type TodoStatus = 'todo' | 'in_progress' | 'done';

export type RepeatType = 'none' | 'daily' | 'weekly' | 'monthly';
export type FocusTimerMode = 'countdown' | 'countup';

export interface TodoItem {
  id: string;
  title: string;
  type: TodoType;
  completed: boolean;
  createdAt: string;
  completedAt?: string;
  order: number;
  notes?: string;
  priority?: 'low' | 'medium' | 'high';
  dueDate?: string;
  subTasks?: TodoSubTask[];
  repeat?: RepeatType;
  estimatedMinutes?: number;
  focusTimerMode?: FocusTimerMode;
  focusMinutes?: number;
  linkedChatSessionIds?: string[];
  linkedAgentSessionIds?: string[];
  planningState?: 'inbox' | 'planned';
  origin?: 'quick_capture' | string;
  updatedAt?: string;
  learningIntent?: boolean;
  deliverableUrl?: string;
  status?: TodoStatus; // kanban status
  progress?: number;
  goalId?: string;
  keyResultId?: string;
  executionWorkspace?: {
    path: string;
    boundAt: string;
  };
}

export interface KeyResult {
  id: string;
  code: string;
  title: string;
  status?: 'active' | 'achieved' | 'abandoned';
}

export interface Goal {
  id: string;
  title: string;
  description?: string;
  emoji?: string;
  deadline?: string;       // 如 "2026-12"
  periodStart?: string;
  periodEnd?: string;
  keyResults?: KeyResult[];
  status: 'active' | 'achieved' | 'abandoned';
  createdAt: string;
  updatedAt: string;
}
