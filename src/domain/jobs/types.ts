export interface JobInfo {
  name: string;
  type: 'command' | 'agent';
  enabled: boolean;
  cron: string;
  cronHuman: string;
  command: string | null;
  prompt: string | null;
  model: string | null;
  cwd: string | null;
  group: string | null;
  onMissed: 'run_once' | 'skip';
  status: 'idle' | 'running' | 'error' | 'disabled';
  lastRun: {
    time: string;
    exitCode: number;
    duration: number;
  } | null;
  nextRun: string | null;
  running: {
    startedAt: string;
    pid: number;
  } | null;
  verification: 'verified' | 'legacy';
  verifiedAt: string | null;
  specHash: string | null;
}
