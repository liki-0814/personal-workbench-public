export interface MoaModelRef {
  provider: string;
  model: string;
}

export interface MoaPreset {
  enabled: boolean;
  advisors: MoaModelRef[];
  judge: MoaModelRef;
  imageDescriber?: MoaModelRef;
  advisorMaxTokens?: number | null;
  advisorTemperature?: number | null;
  judgeTemperature?: number | null;
  lowRiskAdvisors?: number | null;
  elevatedRiskAdvisors?: number | null;
  highRiskAdvisors?: number | null;
}

export interface MoaConfig {
  activePreset: string;
  presets: Record<string, MoaPreset>;
}

export const DEFAULT_MOA_CONFIG: MoaConfig = {
  activePreset: 'fusion',
  presets: {
    fusion: {
      enabled: false,
      advisors: [],
      judge: { provider: '', model: '' },
    },
  },
};
