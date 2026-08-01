import { useEffect, useId, useRef, useState } from 'react';
import {
  Activity,
  Check,
  ChevronUp,
  CloudSun,
  MoreHorizontal,
  Pause,
  Play,
  RotateCcw,
  Settings2,
  SkipForward,
  Target,
  X,
} from 'lucide-react';
import { useClock, type City, type WeatherData } from '@/shell';
import { BackgroundTaskPanel, type BackgroundTaskInfo } from '@/domain/chat';

type PomodoroMode = 'work' | 'shortBreak' | 'longBreak';
type TimerSettings = Record<PomodoroMode, number>;

interface BackgroundTasks {
  allTasks: BackgroundTaskInfo[];
  cancel: (id: string) => Promise<void>;
  dismiss: (id: string) => void;
  runningCount: number;
  activeCount: number;
}

interface BindableTask {
  id: string;
  title: string;
}

interface Props {
  taskTitle: string | null;
  boundTaskId: string | null;
  tasks: BindableTask[];
  onBindTask: (taskId: string | null) => void;
  onOpenTasks: () => void;
  timerActive: boolean;
  mode: PomodoroMode;
  timerDirection: 'countdown' | 'countup';
  timeLeft: number;
  isRunning: boolean;
  timerSettings: TimerSettings;
  onStart: () => void;
  onToggle: () => void;
  onSkip: () => void;
  onReset: () => void;
  onUpdateSettings: (updates: Partial<TimerSettings>) => void;
  formatTime: (seconds: number) => string;
  backgroundTasks: BackgroundTasks;
  weather: WeatherData | null;
  weatherLoading: boolean;
  cities: City[];
  selectedCity: City;
  onCityChange: (city: City) => void;
}

const MODE_LABEL: Record<PomodoroMode, string> = {
  work: '专注',
  shortBreak: '短休',
  longBreak: '长休',
};

export default function WorkStatusBar({
  taskTitle,
  boundTaskId,
  tasks,
  onBindTask,
  onOpenTasks,
  timerActive,
  mode,
  timerDirection,
  timeLeft,
  isRunning,
  timerSettings,
  onStart,
  onToggle,
  onSkip,
  onReset,
  onUpdateSettings,
  formatTime,
  backgroundTasks,
  weather,
  weatherLoading,
  cities,
  selectedCity,
  onCityChange,
}: Props) {
  const clock = useClock();
  const [taskBindingOpen, setTaskBindingOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [tasksOpen, setTasksOpen] = useState(false);
  const [weatherOpen, setWeatherOpen] = useState(false);
  const [timerMenuOpen, setTimerMenuOpen] = useState(false);
  const [draft, setDraft] = useState(() => ({
    work: Math.round(timerSettings.work / 60),
    shortBreak: Math.round(timerSettings.shortBreak / 60),
    longBreak: Math.round(timerSettings.longBreak / 60),
  }));
  const taskBindingRef = useRef<HTMLDivElement>(null);
  const settingsRef = useRef<HTMLDivElement>(null);
  const weatherRef = useRef<HTMLDivElement>(null);
  const timerMenuRef = useRef<HTMLDivElement>(null);
  const backgroundTaskTriggerRef = useRef<HTMLButtonElement>(null);
  const backgroundTaskPanelId = useId();

  useEffect(() => {
    if (!taskBindingOpen && !settingsOpen && !tasksOpen && !weatherOpen && !timerMenuOpen) return;
    const handlePointer = (event: MouseEvent) => {
      const target = event.target as Node;
      if (taskBindingOpen && taskBindingRef.current && !taskBindingRef.current.contains(target)) setTaskBindingOpen(false);
      if (settingsOpen && settingsRef.current && !settingsRef.current.contains(target)) setSettingsOpen(false);
      if (weatherOpen && weatherRef.current && !weatherRef.current.contains(target)) setWeatherOpen(false);
      if (timerMenuOpen && timerMenuRef.current && !timerMenuRef.current.contains(target)) setTimerMenuOpen(false);
    };
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setTaskBindingOpen(false);
        setSettingsOpen(false);
        setTasksOpen(false);
        setWeatherOpen(false);
        setTimerMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', handlePointer);
    document.addEventListener('keydown', handleKey);
    return () => {
      document.removeEventListener('mousedown', handlePointer);
      document.removeEventListener('keydown', handleKey);
    };
  }, [taskBindingOpen, settingsOpen, tasksOpen, weatherOpen, timerMenuOpen]);

  const openSettings = () => {
    setDraft({
      work: Math.round(timerSettings.work / 60),
      shortBreak: Math.round(timerSettings.shortBreak / 60),
      longBreak: Math.round(timerSettings.longBreak / 60),
    });
    setSettingsOpen(value => !value);
  };

  const saveSettings = () => {
    onUpdateSettings({
      work: Math.max(60, draft.work * 60),
      shortBreak: Math.max(60, draft.shortBreak * 60),
      longBreak: Math.max(60, draft.longBreak * 60),
    });
    setSettingsOpen(false);
    setTimerMenuOpen(false);
  };

  const openTimerMenu = () => {
    setDraft({
      work: Math.round(timerSettings.work / 60),
      shortBreak: Math.round(timerSettings.shortBreak / 60),
      longBreak: Math.round(timerSettings.longBreak / 60),
    });
    setTimerMenuOpen(value => !value);
  };

  return (
    <footer className="work-status-bar">
      <div
        className="workbar-popover"
        ref={taskBindingRef}
        style={{ minWidth: 0, maxWidth: 'min(310px, 45vw)' }}
      >
        <button
          type="button"
          className="work-status-task"
          onClick={() => setTaskBindingOpen(value => !value)}
          title="绑定或切换当前任务"
          aria-expanded={taskBindingOpen}
          style={{ width: '100%' }}
        >
          <Target size={14} strokeWidth={1.8} />
          <span className="work-status-label">当前任务</span>
          <strong>{taskTitle || '点击绑定任务'}</strong>
          <ChevronUp size={11} />
        </button>
        {taskBindingOpen && (
          <div
            className="status-popover weather-popover"
            style={{ left: 0, right: 'auto', width: 'min(320px, calc(100vw - 20px))', maxHeight: 'min(420px, calc(100dvh - 72px))', overflowY: 'auto' }}
          >
            <div className="status-popover-heading">
              <strong>绑定专注任务</strong>
              <button type="button" onClick={() => setTaskBindingOpen(false)} aria-label="关闭任务选择">
                <X size={14} />
              </button>
            </div>
            {boundTaskId && (
              <button
                type="button"
                onClick={() => { onBindTask(null); setTaskBindingOpen(false); }}
              >
                不绑定任务
              </button>
            )}
            {tasks.length > 0 ? tasks.map(task => (
              <button
                type="button"
                key={task.id}
                className={boundTaskId === task.id ? 'is-active' : ''}
                onClick={() => { onBindTask(task.id); setTaskBindingOpen(false); }}
                aria-pressed={boundTaskId === task.id}
                title={task.title}
                style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
              >
                {task.title}
              </button>
            )) : (
              <div className="px-2 py-3 text-center text-[11px]" style={{ color: 'var(--text-muted)' }}>
                暂无可绑定任务
              </div>
            )}
            <button
              type="button"
              className="status-popover-primary"
              onClick={() => { setTaskBindingOpen(false); onOpenTasks(); }}
            >
              打开任务管理
            </button>
          </div>
        )}
      </div>

      <div className="work-status-divider" />

      <div className="work-status-timer">
        <span className={`work-status-dot ${isRunning ? 'is-live' : ''}`} />
        <span className="work-status-label">{timerActive ? MODE_LABEL[mode] : '专注'}</span>
        <strong className="work-status-time">
          {timerActive ? formatTime(timeLeft) : formatTime(timerSettings.work)}
        </strong>
        <div className="work-status-controls">
          <button
            type="button"
            onClick={timerActive ? onToggle : onStart}
            aria-label={timerActive ? (isRunning ? '暂停计时' : '继续计时') : '开始专注'}
            title={timerActive ? (isRunning ? '暂停' : '继续') : '开始专注'}
          >
            {timerActive && isRunning ? <Pause size={13} fill="currentColor" /> : <Play size={13} fill="currentColor" />}
          </button>
          {timerActive && (
            <>
              <button
                type="button"
                onClick={onSkip}
                aria-label={timerDirection === 'countup' && mode === 'work' ? '结束并记录' : '跳过'}
                title={timerDirection === 'countup' && mode === 'work' ? '结束并记录' : '跳过'}
              >
                {timerDirection === 'countup' && mode === 'work' ? <Check size={13} /> : <SkipForward size={13} />}
              </button>
              <button type="button" onClick={onReset} aria-label="重置计时" title="重置">
                <RotateCcw size={13} />
              </button>
            </>
          )}
          <div className="workbar-popover" ref={settingsRef}>
            <button type="button" onClick={openSettings} aria-label="番茄钟设置" title="时长设置">
              <Settings2 size={13} />
            </button>
            {settingsOpen && (
              <div className="status-popover timer-settings-popover">
                <div className="status-popover-heading"><strong>番茄钟时长</strong><button type="button" onClick={() => setSettingsOpen(false)} aria-label="关闭时长设置"><X size={14} /></button></div>
                {([
                  ['work', '专注'],
                  ['shortBreak', '短休'],
                  ['longBreak', '长休'],
                ] as const).map(([key, label]) => (
                  <label key={key} className="timer-setting-row">
                    <span>{label}</span>
                    <input
                      type="number"
                      min={1}
                      max={120}
                      value={draft[key]}
                      onChange={event => setDraft(value => ({ ...value, [key]: Math.max(1, Math.min(120, Number(event.target.value) || 1)) }))}
                    />
                    <small>分钟</small>
                  </label>
                ))}
                <button type="button" className="status-popover-primary" onClick={saveSettings}>保存时长</button>
              </div>
            )}
          </div>
        </div>
        <div className="workbar-popover mobile-timer-menu" ref={timerMenuRef}>
          <button
            type="button"
            className="mobile-timer-trigger"
            onClick={openTimerMenu}
            aria-label="更多计时操作"
            aria-expanded={timerMenuOpen}
          >
            <MoreHorizontal size={14} />
          </button>
          {timerMenuOpen && (
            <div className="status-popover mobile-timer-popover">
              <div className="status-popover-heading">
                <strong>计时操作</strong>
                <button type="button" onClick={() => setTimerMenuOpen(false)} aria-label="关闭计时操作"><X size={14} /></button>
              </div>
              {timerActive && (
                <div className="mobile-timer-actions">
                  <button
                    type="button"
                    onClick={() => { onSkip(); setTimerMenuOpen(false); }}
                  >
                    {timerDirection === 'countup' && mode === 'work' ? <Check size={14} /> : <SkipForward size={14} />}
                    {timerDirection === 'countup' && mode === 'work' ? '结束并记录' : '跳过当前阶段'}
                  </button>
                  <button type="button" onClick={() => { onReset(); setTimerMenuOpen(false); }}>
                    <RotateCcw size={14} />
                    重置计时
                  </button>
                </div>
              )}
              <div className="mobile-timer-settings-title">时长设置</div>
              {([
                ['work', '专注'],
                ['shortBreak', '短休'],
                ['longBreak', '长休'],
              ] as const).map(([key, label]) => (
                <label key={key} className="timer-setting-row">
                  <span>{label}</span>
                  <input
                    type="number"
                    min={1}
                    max={120}
                    value={draft[key]}
                    onChange={event => setDraft(value => ({ ...value, [key]: Math.max(1, Math.min(120, Number(event.target.value) || 1)) }))}
                  />
                  <small>分钟</small>
                </label>
              ))}
              <button type="button" className="status-popover-primary" onClick={saveSettings}>保存时长</button>
            </div>
          )}
        </div>
      </div>

      <div className="work-status-spacer" />

      <div className="workbar-popover background-task-popover">
        <button
          ref={backgroundTaskTriggerRef}
          type="button"
          className={`work-status-compact ${backgroundTasks.runningCount > 0 ? 'is-live' : ''}`}
          onClick={() => setTasksOpen(value => !value)}
          title={`${backgroundTasks.runningCount} 个运行中 / ${backgroundTasks.activeCount} 个任务`}
          aria-expanded={tasksOpen}
          aria-haspopup="dialog"
          aria-controls={tasksOpen ? backgroundTaskPanelId : undefined}
        >
          <Activity size={14} />
          <span>AI</span>
          <strong>{backgroundTasks.activeCount}</strong>
        </button>
        {tasksOpen && (
          <BackgroundTaskPanel
            id={backgroundTaskPanelId}
            anchorRef={backgroundTaskTriggerRef}
            tasks={backgroundTasks.allTasks}
            onCancel={backgroundTasks.cancel}
            onDismiss={backgroundTasks.dismiss}
            onClose={() => setTasksOpen(false)}
          />
        )}
      </div>

      <div className="work-status-divider ambient-divider" />

      <div className="workbar-popover" ref={weatherRef}>
        <button
          type="button"
          className="work-status-weather"
          onClick={() => setWeatherOpen(value => !value)}
          title="切换天气城市"
          aria-expanded={weatherOpen}
          aria-busy={weatherLoading}
        >
          {weather ? <span aria-hidden="true">{weather.icon}</span> : <CloudSun size={14} />}
          <strong>{weatherLoading ? '--' : weather?.temp || '--'}</strong>
          <small>{weather?.city || selectedCity.name}</small>
          <ChevronUp size={11} />
        </button>
        {weatherOpen && (
          <div className="status-popover weather-popover">
            <div className="status-popover-heading"><strong>天气城市</strong><button type="button" onClick={() => setWeatherOpen(false)} aria-label="关闭天气城市选择"><X size={14} /></button></div>
            {cities.map(city => (
              <button
                type="button"
                key={city.name}
                className={selectedCity.name === city.name ? 'is-active' : ''}
                onClick={() => { onCityChange(city); setWeatherOpen(false); }}
              >
                {city.name}
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="work-status-clock" aria-label={`${clock.date} ${clock.weekday} ${clock.time}`}>
        <span>{clock.date} · {clock.weekday}</span>
        <strong>{clock.time}</strong>
      </div>
    </footer>
  );
}
