import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SettingsModal from './SettingsModal';

const mocks = vi.hoisted(() => ({
  apiFetch: vi.fn(),
  saveLocalConfig: vi.fn(),
  setAppConfig: vi.fn(),
  showToast: vi.fn(),
  appConfig: {},
  moaConfig: {},
  localConfig: {
    tools: { fsBase: '' },
    ai: {},
    permissions: { allow_rules: [] },
  },
}));

vi.mock('sortablejs', () => ({
  default: class {
    destroy() {}
  },
}));

vi.mock('@/core/config', () => ({
  FEATURE_MODELS: [],
  LOCAL_CONFIG_TOKEN_MASK: '******',
  getFeatureModel: vi.fn(() => ''),
  getMoaConfig: vi.fn(() => mocks.moaConfig),
  getProviderModelOptions: vi.fn(() => []),
  getProviders: vi.fn(() => []),
  normalizeFsBase: (value?: string) => value?.trim() || '~/',
  saveLocalConfig: mocks.saveLocalConfig,
  saveMoaConfig: vi.fn(),
  setAppConfig: mocks.setAppConfig,
  setFeatureModel: vi.fn(),
  setProviders: vi.fn(),
  supportsKimiDeferredTools: vi.fn(() => false),
  useAiModels: vi.fn(() => []),
  useAppConfig: vi.fn(() => mocks.appConfig),
  useLocalConfig: vi.fn(() => mocks.localConfig),
  useMoaConfig: vi.fn(() => mocks.moaConfig),
  validateMoaConfig: vi.fn(() => []),
}));

vi.mock('@/core/storage', () => ({ useStorageSync: vi.fn() }));
vi.mock('@/core/utils', () => ({ apiFetch: mocks.apiFetch }));
vi.mock('@/features/memory', () => ({ MemorySettingsPanel: () => null }));
vi.mock('./HarnessMoaSettingsPanel', () => ({ default: () => null }));
vi.mock('@/shell/state/mermaidSettings', () => ({
  getMermaidEnabled: vi.fn(() => true),
  setMermaidEnabled: vi.fn(),
}));
vi.mock('@/shell', () => ({
  SelectField: () => null,
  showToast: mocks.showToast,
  useModalDialog: () => ({ current: null }),
}));

describe('SettingsModal fsBase', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    mocks.apiFetch.mockReset();
    mocks.apiFetch.mockImplementation(async path => {
      if (path === '/api/agent/daemon/status') {
        return { healthy: true, version: 'test', httpAddress: '127.0.0.1:3456' };
      }
      if (path.startsWith('/api/agent/backends')) return [];
      throw new Error(`Unexpected request: ${path}`);
    });
    mocks.saveLocalConfig.mockReset();
    mocks.saveLocalConfig.mockResolvedValue(undefined);
    mocks.setAppConfig.mockReset();
    mocks.showToast.mockReset();
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it('将旧的空配置显示并保存为字面量 ~/', async () => {
    await act(async () => root.render(<SettingsModal open onClose={vi.fn()} />));

    const integrationsTab = Array.from(container.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.textContent === '工具与权限');
    await act(async () => integrationsTab?.click());

    const fsBaseInput = container.querySelector<HTMLInputElement>('[aria-label="允许访问的根目录"]');
    expect(fsBaseInput?.value).toBe('~/');

    const saveButton = Array.from(container.querySelectorAll<HTMLButtonElement>('button'))
      .find(button => button.textContent === '保存');
    expect(saveButton?.disabled).toBe(false);
    await act(async () => saveButton?.click());

    expect(mocks.saveLocalConfig).toHaveBeenCalledOnce();
    expect(mocks.saveLocalConfig.mock.calls[0][0]).toMatchObject({
      tools: { fsBase: '~/' },
    });
    expect(mocks.showToast).toHaveBeenCalledWith({
      message: '已保存。所有字段立即生效，无需重启',
      type: 'success',
    });
  });
});
