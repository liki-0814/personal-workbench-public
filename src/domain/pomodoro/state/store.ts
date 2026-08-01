import { useState, useEffect, useCallback, useRef } from 'react';
import { load, save, KEYS } from '@/core/storage';
import { formatDateTime } from '@/core/utils/date';
import type { PomodoroRecord } from '../types';

type TimerMode = 'work' | 'shortBreak' | 'longBreak';
type TimerDirection = 'countdown' | 'countup';

const DEFAULT_SETTINGS: Record<TimerMode, number> = {
  work: 25 * 60,
  shortBreak: 5 * 60,
  longBreak: 15 * 60,
};

const MODE_LABELS = {
  work: '专注',
  shortBreak: '短休息',
  longBreak: '长休息',
};

interface PomodoroState {
  todaySessions: number;
  lastSessionDate: string; // YYYY-MM-DD
}

function getTodayStr() {
  return new Date().toISOString().slice(0, 10);
}

/** 完成一个专注时段，返回下一个状态 */
function completeWorkSession(
  sessions: number,
  pomodoroState: PomodoroState,
  startTimeRef: { current: string | null },
  onWorkComplete: ((record: Omit<PomodoroRecord, 'id'>) => void) | undefined,
  boundTodoId: string | null | undefined,
  boundTodoTitle: string | undefined,
  duration: number,
): { newSessions: number; nextState: PomodoroState; nextMode: TimerMode } {
  const newSessions = sessions + 1;
  const todayStr = getTodayStr();
  const nextState: PomodoroState = {
    todaySessions: pomodoroState.lastSessionDate === todayStr
      ? pomodoroState.todaySessions + 1
      : 1,
    lastSessionDate: todayStr,
  };

  if (startTimeRef.current && onWorkComplete) {
    const endTime = formatDateTime(new Date());
    onWorkComplete({
      todoId: boundTodoId || undefined,
      todoTitle: boundTodoTitle || undefined,
      mode: 'work',
      duration,
      startTime: startTimeRef.current,
      endTime,
    });
  }
  startTimeRef.current = null;

  const nextMode = newSessions % 4 === 0 ? 'longBreak' : 'shortBreak';
  return { newSessions, nextState, nextMode };
}

export function usePomodoro(
  onWorkComplete?: (record: Omit<PomodoroRecord, 'id'>) => void,
  boundTodoId?: string | null,
  boundTodoTitle?: string,
) {
  const [mode, setMode] = useState<TimerMode>('work');
  const [timerDirection, setTimerDirection] = useState<TimerDirection>('countdown');
  const [timerSettings, setTimerSettings] = useState<Record<TimerMode, number>>(() =>
    load(KEYS.POMODORO_SETTINGS, DEFAULT_SETTINGS)
  );
  const [activeWorkSeconds, setActiveWorkSeconds] = useState(timerSettings.work);
  const [timeLeft, setTimeLeft] = useState(timerSettings.work);
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const [isRunning, setIsRunning] = useState(false);
  const [sessions, setSessions] = useState(0);
  const startTimeRef = useRef<string | null>(null);

  const [pomodoroState, setPomodoroState] = useState<PomodoroState>(() => {
    const saved = load<PomodoroState | null>(KEYS.POMODORO, null);
    if (saved && saved.lastSessionDate === getTodayStr()) {
      return saved;
    }
    return { todaySessions: 0, lastSessionDate: getTodayStr() };
  });

  const persistState = useCallback((state: PomodoroState) => {
    setPomodoroState(state);
    save(KEYS.POMODORO, state);
  }, []);

  const updateTimerSettings = useCallback((updates: Partial<Record<TimerMode, number>>) => {
    if (typeof updates.work === 'number') {
      setActiveWorkSeconds(updates.work);
    }
    setTimerSettings(prev => {
      const next = { ...prev, ...updates };
      save(KEYS.POMODORO_SETTINGS, next);
      return next;
    });
  }, []);

  // Reset runtime state when mode changes
  useEffect(() => {
    setIsRunning(false);
    setElapsedSeconds(0);
    startTimeRef.current = null;
    if (mode === 'work') {
      setTimeLeft(activeWorkSeconds);
    } else {
      setTimeLeft(timerSettings[mode]);
    }
  }, [mode]); // eslint-disable-line react-hooks/exhaustive-deps

  // Keep break durations synced with latest settings
  useEffect(() => {
    if (mode !== 'work') {
      setTimeLeft(timerSettings[mode]);
    }
  }, [mode, timerSettings]);

  // Keep work duration changes reflected when idle
  useEffect(() => {
    if (mode === 'work' && !isRunning) {
      setTimeLeft(activeWorkSeconds);
    }
  }, [mode, isRunning, activeWorkSeconds]);

  // Track start time when timer starts in work mode
  useEffect(() => {
    if (isRunning && mode === 'work' && !startTimeRef.current) {
      startTimeRef.current = formatDateTime(new Date());
    }
  }, [isRunning, mode]);

  // 倒计时 tick
  useEffect(() => {
    if (!isRunning || timerDirection !== 'countdown' || timeLeft <= 0) return;

    const interval = setInterval(() => {
      setTimeLeft(prev => {
        if (prev <= 1) {
          setIsRunning(false);
          // Play notification sound
          try {
            const audio = new Audio('data:audio/wav;base64,UklGRnoGAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YQoGAACBhYqFbF1fdJivrJBhNjVgodDbq2EcBj+a2teleQkANJaqrHJMCAc8mu3Rfm4TAB1VwIhxFAQHHmPDjn4gCwAeTruseS8KAB5Lvu99IAoAHki/8JUgCQAeSL/wniAJAB5IwDCfIAkAHkjBMaAgCQAeSMQypSAJAB5IxjSnIAkAHknHPNQgCQAeSckA2yAJAB5JyQbfIAkAHknJBt8gCQAeSckG3yAJAB5JyQbfIAkAHknJBt8gCQAeSckG3yAJAB5JyQbfIAkA');
            audio.play().catch(() => { /* noop */ });
          } catch { /* noop */ }

          // Auto switch mode
          if (mode === 'work') {
            const { newSessions, nextState, nextMode } = completeWorkSession(
              sessions, pomodoroState, startTimeRef, onWorkComplete,
              boundTodoId, boundTodoTitle, activeWorkSeconds,
            );
            setSessions(newSessions);
            persistState(nextState);
            setMode(nextMode);
          } else {
            setMode('work');
          }
          return 0;
        }
        return prev - 1;
      });
    }, 1000);

    return () => clearInterval(interval);
  }, [isRunning, timerDirection, timeLeft, mode, sessions, pomodoroState, persistState, onWorkComplete, boundTodoId, boundTodoTitle, activeWorkSeconds]);

  // 正计时 tick
  useEffect(() => {
    if (!isRunning || mode !== 'work' || timerDirection !== 'countup') return;
    const interval = setInterval(() => {
      setElapsedSeconds(prev => prev + 1);
    }, 1000);
    return () => clearInterval(interval);
  }, [isRunning, mode, timerDirection]);

  const toggle = useCallback(() => {
    setIsRunning(prev => !prev);
  }, []);

  const reset = useCallback(() => {
    setTimeLeft(mode === 'work' ? activeWorkSeconds : timerSettings[mode]);
    setElapsedSeconds(0);
    setTimerDirection('countdown');
    setIsRunning(false);
    startTimeRef.current = null;
  }, [mode, timerSettings, activeWorkSeconds]);

  const completeCurrent = useCallback(() => {
    if (mode === 'work') {
      const duration = timerDirection === 'countup'
        ? Math.max(1, elapsedSeconds)
        : (() => {
            const elapsed = activeWorkSeconds - timeLeft;
            return elapsed > 0 ? elapsed : activeWorkSeconds;
          })();
      const { newSessions, nextState, nextMode } = completeWorkSession(
        sessions, pomodoroState, startTimeRef, onWorkComplete,
        boundTodoId, boundTodoTitle, duration,
      );
      setSessions(newSessions);
      persistState(nextState);
      setMode(nextMode);
    } else {
      setMode('work');
    }
    setElapsedSeconds(0);
    setTimerDirection('countdown');
    setIsRunning(false);
  }, [mode, timerDirection, elapsedSeconds, sessions, pomodoroState, persistState, onWorkComplete, boundTodoId, boundTodoTitle, activeWorkSeconds, timeLeft]);

  const skip = useCallback(() => {
    completeCurrent();
  }, [completeCurrent]);

  const switchMode = useCallback((newMode: TimerMode) => {
    setMode(newMode);
    setTimerDirection('countdown');
    setElapsedSeconds(0);
    setIsRunning(false);
    startTimeRef.current = null;
  }, []);

  const startWorkSession = useCallback((options?: { direction?: TimerDirection; workSeconds?: number }) => {
    const direction = options?.direction ?? 'countdown';
    const workSeconds = Math.max(60, options?.workSeconds ?? activeWorkSeconds);
    setMode('work');
    setTimerDirection(direction);
    setElapsedSeconds(0);
    setActiveWorkSeconds(workSeconds);
    setTimeLeft(workSeconds);
    setIsRunning(true);
  }, [activeWorkSeconds]);

  const formatTime = useCallback((seconds: number) => {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  }, []);

  return {
    mode,
    timerDirection,
    timeLeft,
    elapsedSeconds,
    displaySeconds: timerDirection === 'countup' && mode === 'work' ? elapsedSeconds : timeLeft,
    totalSeconds: mode === 'work' ? activeWorkSeconds : timerSettings[mode],
    isRunning,
    sessions,
    todaySessions: pomodoroState.todaySessions,
    label: MODE_LABELS[mode],
    timerSettings,
    toggle,
    reset,
    skip,
    completeCurrent,
    switchMode,
    startWorkSession,
    formatTime,
    updateTimerSettings,
  };
}
