export interface PomodoroRecord {
  id: string;
  todoId?: string;
  todoTitle?: string;
  mode: 'work' | 'shortBreak' | 'longBreak';
  duration: number; // seconds
  startTime: string; // yyyyMMdd hh:mm:ss
  endTime: string; // yyyyMMdd hh:mm:ss
}
