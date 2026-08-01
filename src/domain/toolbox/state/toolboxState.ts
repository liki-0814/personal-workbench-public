import { useCallback, useEffect, useRef, useState } from 'react';
import { KEYS, load, save, useStorageSync } from '@/core/storage';
import type { ToolboxState } from '../types';

const DEFAULT_TOOLBOX_STATE: ToolboxState = {
  activeTool: 'clipboard',
  diff: {
    original: '',
    changed: '',
    merged: '',
    layout: 'side-by-side',
    ignoreWhitespace: false,
    choices: {},
    activeChange: 0,
  },
  timestamp: {
    input: '',
    unit: 'auto',
  },
};

function normalizeState(value: Partial<ToolboxState> | undefined): ToolboxState {
  return {
    ...DEFAULT_TOOLBOX_STATE,
    ...value,
    diff: { ...DEFAULT_TOOLBOX_STATE.diff, ...value?.diff },
    timestamp: { ...DEFAULT_TOOLBOX_STATE.timestamp, ...value?.timestamp },
  };
}

export function useToolboxState() {
  const [state, setState] = useState<ToolboxState>(() =>
    normalizeState(load<Partial<ToolboxState>>(KEYS.TOOLBOX_STATE, DEFAULT_TOOLBOX_STATE)),
  );
  const stateRef = useRef(state);
  const didChangeRef = useRef(false);
  stateRef.current = state;

  useEffect(() => {
    if (!didChangeRef.current) return;
    const timer = window.setTimeout(() => save(KEYS.TOOLBOX_STATE, state), 350);
    return () => window.clearTimeout(timer);
  }, [state]);

  useEffect(() => () => {
    if (didChangeRef.current) {
      save(KEYS.TOOLBOX_STATE, stateRef.current);
    }
  }, []);

  useStorageSync(() => {
    const next = normalizeState(
      load<Partial<ToolboxState>>(KEYS.TOOLBOX_STATE, DEFAULT_TOOLBOX_STATE),
    );
    if (JSON.stringify(next) !== JSON.stringify(stateRef.current)) {
      stateRef.current = next;
      setState(next);
    }
  });

  const update = useCallback((updater: (current: ToolboxState) => ToolboxState) => {
    setState(current => {
      const next = updater(current);
      didChangeRef.current = true;
      stateRef.current = next;
      return next;
    });
  }, []);

  return { state, update };
}
