import { useMemo } from 'react';
import { Clock3, Copy, RefreshCw } from 'lucide-react';
import { SelectField, showToast } from '@/shell';
import type { TimestampToolState } from '../../types';
import { parseTimestamp } from '../../utils/timestamp';

interface Props {
  state: TimestampToolState;
  onChange: (state: TimestampToolState) => void;
}

const UNITS: Array<{ value: TimestampToolState['unit']; label: string }> = [
  { value: 'auto', label: '自动识别' },
  { value: 'seconds', label: '秒' },
  { value: 'milliseconds', label: '毫秒' },
  { value: 'microseconds', label: '微秒' },
  { value: 'nanoseconds', label: '纳秒' },
];

export default function TimestampTool({ state, onChange }: Props) {
  const result = useMemo(() => parseTimestamp(state.input, state.unit), [state.input, state.unit]);
  const invalid = state.input.trim().length > 0 && !result;

  const copy = async (value: string, label: string) => {
    await navigator.clipboard.writeText(value);
    showToast({ message: `${label}已复制`, type: 'success' });
  };

  const rows = result ? [
    ['本地时间', result.local],
    ['UTC', result.utc],
    ['ISO 8601', result.iso],
    ['Unix 秒', result.seconds],
    ['Unix 毫秒', result.milliseconds],
  ] as const : [];

  return (
    <div className="timestamp-tool">
      <div className="timestamp-hero">
        <div className="timestamp-orbit" aria-hidden="true"><Clock3 size={25} /></div>
        <div>
          <span className="tool-eyebrow">Timestamp converter</span>
          <h2>时间戳转换</h2>
          <p>在秒、毫秒、微秒和纳秒时间戳之间自动识别，并显示常用时间格式。</p>
        </div>
      </div>

      <div className="timestamp-input-card">
        <label htmlFor="timestamp-input">时间戳</label>
        <div className="timestamp-input-row">
          <input
            id="timestamp-input"
            value={state.input}
            onChange={event => onChange({ ...state, input: event.target.value })}
            placeholder="例如 1784106133000"
            inputMode="numeric"
            spellCheck={false}
            aria-invalid={invalid}
          />
          <SelectField
            value={state.unit}
            options={UNITS}
            onValueChange={value => onChange({ ...state, unit: value as TimestampToolState['unit'] })}
            ariaLabel="时间戳单位"
            density="compact"
            className="timestamp-unit-select"
          />
          <button type="button" onClick={() => onChange({ ...state, input: Date.now().toString(), unit: 'milliseconds' })}>
            <RefreshCw size={14} />当前时间
          </button>
        </div>
        {invalid && <p className="timestamp-error">请输入有效的整数时间戳，或检查所选单位。</p>}
      </div>

      <div className="timestamp-results" aria-live="polite">
        {rows.length > 0 ? rows.map(([label, value]) => (
          <div key={label} className="timestamp-result-row">
            <span>{label}</span>
            <code>{value}</code>
            <button type="button" onClick={() => copy(value, label)} aria-label={`复制${label}`}><Copy size={14} /></button>
          </div>
        )) : (
          <div className="timestamp-empty">
            <Clock3 size={20} />
            <span>输入时间戳后，转换结果会显示在这里。</span>
          </div>
        )}
      </div>
    </div>
  );
}
