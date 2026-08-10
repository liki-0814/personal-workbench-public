import { useMemo } from 'react';
import { useChatModels } from '@/core/config/hooks';
import { modelSelectionKey, resolveModelSelection } from '@/core/config/aiProviders';
import { SelectField } from '@/shell';
import type { AiModel } from '../../types';

interface Props {
  value: AiModel;
  onChange: (m: AiModel) => void;
  className?: string;
  size?: 'sm' | 'md';
}

const MENU_MIN_WIDTH = 220;

export default function ModelSelector({ value, onChange, className, size = 'md' }: Props) {
  const chatModels = useChatModels();
  const modelOptions = useMemo(
    () => chatModels.map(m => ({ value: modelSelectionKey(m), label: m.name, group: m.providerName ?? 'Models' })),
    [chatModels]
  );

  const resolvedValue = useMemo(() => {
    const selection = resolveModelSelection(value);
    const byId = modelOptions.find(m => m.value === selection);
    if (byId) return byId.value;
    const byLabel = modelOptions.find(m => m.label === value);
    return byLabel?.value ?? value;
  }, [modelOptions, value]);

  const isSm = size === 'sm';
  const selectedLabel = modelOptions.find(m => m.value === resolvedValue)?.label ?? value;

  return (
    <div className={`relative ${className || ''}`}>
      <SelectField
        value={resolvedValue}
        options={modelOptions}
        onValueChange={next => onChange(next)}
        density={isSm ? 'compact' : 'default'}
        className={`precision-model-trigger ${isSm ? 'text-xs' : 'text-sm w-full min-w-[130px]'}`}
        menuMinWidth={MENU_MIN_WIDTH}
        menuClassName="precision-model-menu"
        optionClassName="precision-model-option"
        ariaLabel={`切换模型，当前为 ${selectedLabel}`}
      />
    </div>
  );
}
