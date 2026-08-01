import type { Goal, KeyResult, TodoItem } from '../types';

export type TaskFilter = 'all' | 'in_progress' | 'completed';

export interface KeyResultGroup {
  keyResult: KeyResult | null;
  todos: TodoItem[];
  completed: number;
  total: number;
}

export interface ObjectiveGroup {
  goal: Goal | null;
  keyResults: KeyResultGroup[];
  completed: number;
  total: number;
}

export function matchesTaskFilter(todo: TodoItem, filter: TaskFilter): boolean {
  if (filter === 'all') return true;
  if (filter === 'completed') return todo.completed || todo.status === 'done';
  return !todo.completed && todo.status !== 'done';
}

export function groupTodosByObjective(
  todos: TodoItem[],
  goals: Goal[],
  filter: TaskFilter,
  selectedGoalId: string | null = null,
): ObjectiveGroup[] {
  const visible = todos.filter(todo => matchesTaskFilter(todo, filter));
  const knownGoalIds = new Set(goals.map(goal => goal.id));
  const selectedGoals = selectedGoalId
    ? goals.filter(goal => goal.id === selectedGoalId)
    : goals;
  const groups: ObjectiveGroup[] = selectedGoals.map(goal => {
    const allItems = todos.filter(todo => todo.goalId === goal.id);
    const keyResults = goal.keyResults ?? [];
    const knownKeyResultIds = new Set(keyResults.map(keyResult => keyResult.id));
    const resultGroups: KeyResultGroup[] = keyResults.map(keyResult => {
      const allResultItems = allItems.filter(todo => todo.keyResultId === keyResult.id);
      return {
        keyResult,
        todos: visible.filter(todo => todo.goalId === goal.id && todo.keyResultId === keyResult.id),
        completed: allResultItems.filter(todo => todo.completed || todo.status === 'done').length,
        total: allResultItems.length,
      };
    });
    const allWithoutResult = allItems.filter(todo => !todo.keyResultId || !knownKeyResultIds.has(todo.keyResultId));
    const visibleWithoutResult = visible.filter(todo => todo.goalId === goal.id && (!todo.keyResultId || !knownKeyResultIds.has(todo.keyResultId)));
    if (visibleWithoutResult.length > 0) {
      resultGroups.push({
        keyResult: null,
        todos: visibleWithoutResult,
        completed: allWithoutResult.filter(todo => todo.completed || todo.status === 'done').length,
        total: allWithoutResult.length,
      });
    }
    return {
      goal,
      keyResults: resultGroups,
      completed: allItems.filter(todo => todo.completed || todo.status === 'done').length,
      total: allItems.length,
    };
  });

  if (!selectedGoalId) {
    const unassigned = visible.filter(todo => !todo.goalId || !knownGoalIds.has(todo.goalId));
    const allUnassigned = todos.filter(todo => !todo.goalId || !knownGoalIds.has(todo.goalId));
    if (unassigned.length > 0) {
      groups.push({
        goal: null,
        keyResults: [{
          keyResult: null,
          todos: unassigned,
          completed: allUnassigned.filter(todo => todo.completed || todo.status === 'done').length,
          total: allUnassigned.length,
        }],
        completed: allUnassigned.filter(todo => todo.completed || todo.status === 'done').length,
        total: allUnassigned.length,
      });
    }
  }
  return groups.filter(group => group.keyResults.some(result => result.todos.length > 0));
}
