export type ClipboardItemType = 'text' | 'url' | 'code' | 'image';

export interface ClipboardItem {
  id: string;
  content: string;
  type: ClipboardItemType;
  timestamp: string;
  source?: string;
}

export type ToolboxTool = 'clipboard' | 'diff' | 'timestamp';

export interface DiffMergeState {
  original: string;
  changed: string;
  merged: string;
  layout: 'side-by-side' | 'inline';
  ignoreWhitespace: boolean;
  choices: Record<string, 'left' | 'right'>;
  activeChange: number;
}

export interface TimestampToolState {
  input: string;
  unit: 'auto' | 'seconds' | 'milliseconds' | 'microseconds' | 'nanoseconds';
}

export interface ToolboxState {
  activeTool: ToolboxTool;
  diff: DiffMergeState;
  timestamp: TimestampToolState;
}
