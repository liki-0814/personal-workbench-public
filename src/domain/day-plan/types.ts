export interface DayPlanItem {
  id: string;
  date: string;
  title: string;
  startMinute: number;
  durationMinutes: number;
  sourceTodoId?: string;
  createdAt: string;
  updatedAt: string;
}
