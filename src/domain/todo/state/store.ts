import { useState, useCallback } from 'react';
import type { TodoItem, TodoType, TodoSubTask, Goal } from '../types';
import { load, save, KEYS, useStorageSync } from '@/core/storage';
import { generateId } from '@/core/utils/id';
import { now } from '@/core/utils/date';
import { applyTodoProgress, getTodoProgress } from '../utils/progress';

type LegacyTodoItem = TodoItem & {
  isIntradayPlan?: boolean;
  scheduledStart?: string;
  scheduledEnd?: string;
};

function loadTodos(): TodoItem[] {
  return load<LegacyTodoItem[]>(KEYS.TODOS, [])
    .filter(item => !item.isIntradayPlan)
    .map(item => {
      const { isIntradayPlan: _intraday, scheduledStart: _start, scheduledEnd: _end, ...todo } = item;
      return applyTodoProgress(todo, getTodoProgress(todo));
    });
}

export function useTodos() {
  const [todos, setTodos] = useState<TodoItem[]>(() =>
    loadTodos()
  );
  const [goals, setGoals] = useState<Goal[]>(() =>
    load(KEYS.GOALS, [])
  );

  useStorageSync(() => {
    setTodos(loadTodos());
    setGoals(load(KEYS.GOALS, []));
  });

  const persist = (items: TodoItem[]) => {
    setTodos(items);
    save(KEYS.TODOS, items);
  };

  const addTodo = useCallback((title: string, type: TodoType, notes?: string, priority: TodoItem['priority'] = 'medium', dueDate?: string, subTasks?: TodoSubTask[], goalId?: string, keyResultId?: string) => {
    const maxOrder = todos
      .filter(t => t.type === type && !t.completed)
      .reduce((max, t) => Math.max(max, t.order ?? 0), -1);
    const item: TodoItem = {
      id: generateId(),
      title,
      type,
      completed: false,
      createdAt: now(),
      order: maxOrder + 1,
      notes: notes || '',
      priority: priority || 'medium',
      dueDate: dueDate || '',
      subTasks: subTasks && subTasks.length > 0 ? subTasks : undefined,
      status: 'todo',
      progress: 0,
      goalId,
      keyResultId,
    };
    persist([...todos, item]);
  }, [todos]);

  const toggleTodo = useCallback((id: string) => {
    const todoToToggle = todos.find(t => t.id === id);
    if (!todoToToggle) return;

    // 如果是取消完成（从已完成恢复），需要重新计算 order
    let newOrder = todoToToggle.order;
    if (todoToToggle.completed) {
      // 取消完成：计算该类型下当前最大 order，放到最后
      const maxOrder = todos
        .filter(t => t.type === todoToToggle.type && !t.completed)
        .reduce((max, t) => Math.max(max, t.order ?? 0), -1);
      newOrder = maxOrder + 1;
    }

    persist(todos.map(t =>
      t.id === id
        ? {
            ...applyTodoProgress(t, t.completed ? 0 : 100, now()),
            order: newOrder,
          }
        : t
    ));
  }, [todos]);

  const updateTodo = useCallback((id: string, updates: Partial<Omit<TodoItem, 'id'>>) => {
    persist(todos.map(t =>
      t.id === id
        ? updates.progress === undefined
          ? { ...t, ...updates }
          : applyTodoProgress({ ...t, ...updates }, updates.progress, now())
        : t
    ));
  }, [todos]);

  // 三态循环：已安排未进行 → 进行中 → 已完成 → 已安排未进行
  // 与 completed 字段保持同步：done ⇔ completed=true
  const cycleStatus = useCallback((id: string) => {
    const target = todos.find(t => t.id === id);
    if (!target) return;
    const cur = target.status || (target.completed ? 'done' : 'todo');
    let next: TodoItem['status'];
    let completed = target.completed;
    let completedAt = target.completedAt;
    let order = target.order;
    let progress = getTodoProgress(target);
    if (cur === 'todo') {
      next = 'in_progress';
      progress = Math.max(progress, 1);
    } else if (cur === 'in_progress') {
      next = 'done';
      completed = true;
      completedAt = now();
      progress = 100;
    } else {
      next = 'todo';
      completed = false;
      completedAt = undefined;
      progress = 0;
      // 恢复到队尾
      const maxOrder = todos
        .filter(t => t.type === target.type && !t.completed)
        .reduce((max, t) => Math.max(max, t.order ?? 0), -1);
      order = maxOrder + 1;
    }
    persist(todos.map(t =>
      t.id === id ? { ...t, status: next, completed, completedAt, progress, order } : t
    ));
  }, [todos]);

  const removeTodo = useCallback((id: string) => {
    persist(todos.filter(t => t.id !== id));
  }, [todos]);

  const getActiveTodos = useCallback((type: TodoType) => {
    return todos
      .filter(t => t && t.type === type && !t.completed)
      .sort((a, b) => (a.order ?? 0) - (b.order ?? 0));
  }, [todos]);

  const getCompletedTodos = useCallback((type?: TodoType) => {
    return todos
      .filter(t => t && t.completed && (!type || t.type === type))
      .sort((a, b) => {
        const aTime = a.completedAt ? new Date(a.completedAt).getTime() : 0;
        const bTime = b.completedAt ? new Date(b.completedAt).getTime() : 0;
        return bTime - aTime;
      });
  }, [todos]);

  // ---------- Goals ----------

  const persistGoals = (items: Goal[]) => {
    setGoals(items);
    save(KEYS.GOALS, items);
  };

  const addGoal = useCallback((title: string, description?: string, emoji?: string, deadline?: string, periodStart?: string, periodEnd?: string, keyResults?: Goal['keyResults']) => {
    const item: Goal = {
      id: `goal-${generateId()}`,
      title,
      description: description || undefined,
      emoji: emoji || undefined,
      deadline: deadline || undefined,
      periodStart: periodStart || undefined,
      periodEnd: periodEnd || undefined,
      keyResults: keyResults?.length ? keyResults : undefined,
      status: 'active',
      createdAt: now(),
      updatedAt: now(),
    };
    persistGoals([...goals, item]);
  }, [goals]);

  const updateGoal = useCallback((id: string, patch: Partial<Omit<Goal, 'id'>>) => {
    persistGoals(goals.map(g =>
      g.id === id ? { ...g, ...patch, updatedAt: now() } : g
    ));
  }, [goals]);

  const removeGoal = useCallback((id: string) => {
    persistGoals(goals.filter(g => g.id !== id));
  }, [goals]);

  return { todos, addTodo, toggleTodo, cycleStatus, removeTodo, getActiveTodos, getCompletedTodos, updateTodo, goals, addGoal, updateGoal, removeGoal };
}
