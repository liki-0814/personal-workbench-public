import { useState } from 'react';
import { motion } from 'framer-motion';
import { X, Bot, LoaderCircle, Sparkles, ShieldAlert, CircleAlert } from 'lucide-react';
import { showToast } from '@/shell';
import { generateJobWithAI, type GeneratedJobSpec } from '../../ai/generateJob';
import { proposeJob } from '../../api';

interface Props {
  onClose: () => void;
  onCreated: () => void;
}

const inputClass =
  'w-full px-3 py-1.5 bg-white dark:bg-[#13161C] border border-gray-200 dark:border-[#2A2D35] rounded-[8px] text-[13px] text-gray-900 dark:text-[#E5E7EB] placeholder:text-gray-400 dark:placeholder:text-[#4B5563] outline-none focus:border-gray-300 dark:focus:border-[#3B4049] transition-colors';

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="mb-1 flex items-baseline gap-2 text-[11px] font-medium text-gray-500 dark:text-[#9CA3AF]">
        {label}
        {hint && <span className="font-normal text-gray-400 dark:text-[#4B5563]">{hint}</span>}
      </span>
      {children}
    </label>
  );
}

export default function JobCreateModal({ onClose, onCreated }: Props) {
  const [stage, setStage] = useState<'describe' | 'config'>('describe');
  const [description, setDescription] = useState('');
  const [spec, setSpec] = useState<GeneratedJobSpec | null>(null);
  const [generating, setGenerating] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [approvalReason, setApprovalReason] = useState<string | null>(null);
  const [testFailure, setTestFailure] = useState<string | null>(null);

  const busy = generating || submitting;

  const handleGenerate = async () => {
    const goal = description.trim();
    if (!goal || generating) return;
    setGenerating(true);
    setError(null);
    try {
      const generated = await generateJobWithAI(goal);
      setSpec(generated);
      setStage('config');
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : '生成配置失败');
    } finally {
      setGenerating(false);
    }
  };

  const patchSpec = (patch: Partial<GeneratedJobSpec>) => {
    setSpec(prev => (prev ? { ...prev, ...patch } : prev));
  };

  const handleSubmit = async (approvedTest = false) => {
    if (!spec || submitting) return;
    setSubmitting(true);
    setError(null);
    setApprovalReason(null);
    setTestFailure(null);
    try {
      const result = await proposeJob({ ...spec, approvedTest });
      switch (result.kind) {
        case 'applied':
          showToast({ message: `已创建并启用「${spec.name}」`, type: 'success' });
          onCreated();
          onClose();
          break;
        case 'approval_required':
          setApprovalReason(result.reason);
          break;
        case 'test_failed':
          setTestFailure(result.summary);
          break;
        case 'error':
          setError(result.message);
          break;
      }
    } finally {
      setSubmitting(false);
    }
  };

  const configValid = spec !== null &&
    spec.name.trim().length > 0 &&
    spec.cron.trim().split(/\s+/).length === 5 &&
    (spec.type === 'command' ? Boolean(spec.command?.trim()) : Boolean(spec.prompt?.trim()));

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 backdrop-blur-sm"
      onClick={() => !busy && onClose()}
    >
      <motion.div
        initial={{ opacity: 0, scale: 0.96, y: 10 }}
        animate={{ opacity: 1, scale: 1, y: 0 }}
        transition={{ type: 'spring', stiffness: 320, damping: 26 }}
        className="w-full max-w-[440px] mx-4 rounded-2xl border border-[var(--border-default)] bg-[var(--surface-0)] p-5 shadow-2xl"
        onClick={e => e.stopPropagation()}
      >
        {/* Header */}
        <div className="mb-4 flex items-center justify-between">
          <div className="flex items-center gap-2.5">
            <span className="w-8 h-8 rounded-[10px] flex items-center justify-center text-white shadow-sm" style={{ background: 'var(--chart-gradient-2)' }}>
              <Bot size={15} />
            </span>
            <div>
              <h3 className="text-[15px] font-semibold text-[var(--text-primary)]">AI 创建调度任务</h3>
              <p className="text-[10px] text-[var(--text-secondary)]">真实测试通过后才会启用</p>
            </div>
          </div>
          <button
            onClick={onClose}
            disabled={busy}
            className="w-7 h-7 flex items-center justify-center rounded-[8px] text-[var(--text-secondary)] hover:bg-[var(--surface-2)] transition-colors disabled:opacity-40"
            title="关闭"
          >
            <X size={16} />
          </button>
        </div>

        {stage === 'describe' ? (
          <div className="space-y-3">
            <textarea
              value={description}
              onChange={e => setDescription(e.target.value)}
              placeholder="例如：每天早上 8 点汇总项目根目录的 git 变更，写入日报文件"
              rows={4}
              autoFocus
              disabled={generating}
              className={`${inputClass} resize-none leading-relaxed disabled:opacity-50`}
            />
            {error && (
              <div className="flex items-start gap-2 rounded-[10px] border border-red-200 bg-red-50 px-3 py-2 text-[11px] text-red-700 dark:border-red-900/40 dark:bg-red-950/20 dark:text-red-300">
                <CircleAlert size={13} className="mt-0.5 shrink-0" />
                <span>{error}</span>
              </div>
            )}
            <button
              onClick={() => void handleGenerate()}
              disabled={!description.trim() || generating}
              className="flex h-8 w-full items-center justify-center gap-1.5 rounded-[10px] bg-brand text-[12px] font-medium text-white hover:bg-brand-hover disabled:opacity-40 transition-colors"
            >
              {generating ? <LoaderCircle size={13} className="animate-spin" /> : <Sparkles size={13} />}
              {generating ? '正在生成配置…' : '生成配置'}
            </button>
          </div>
        ) : spec && (
          <div className="space-y-3">
            <div className="grid grid-cols-2 gap-2">
              <Field label="任务名" hint="字母数字 _-.">
                <input value={spec.name} onChange={e => patchSpec({ name: e.target.value })} className={inputClass} />
              </Field>
              <Field label="类型">
                <select
                  value={spec.type}
                  onChange={e => patchSpec({ type: e.target.value as GeneratedJobSpec['type'] })}
                  className={inputClass}
                >
                  <option value="command">命令</option>
                  <option value="agent">AI 执行</option>
                </select>
              </Field>
            </div>
            <Field label="Cron" hint="分 时 日 月 周">
              <input value={spec.cron} onChange={e => patchSpec({ cron: e.target.value })} className={`${inputClass} font-mono`} placeholder="0 8 * * *" />
            </Field>
            {spec.type === 'command' ? (
              <Field label="命令">
                <textarea value={spec.command ?? ''} onChange={e => patchSpec({ command: e.target.value })} rows={2} className={`${inputClass} resize-none font-mono`} />
              </Field>
            ) : (
              <Field label="给 AI 的任务指令">
                <textarea value={spec.prompt ?? ''} onChange={e => patchSpec({ prompt: e.target.value })} rows={3} className={`${inputClass} resize-none`} />
              </Field>
            )}
            <Field label="工作目录" hint="可选">
              <input value={spec.cwd ?? ''} onChange={e => patchSpec({ cwd: e.target.value })} className={`${inputClass} font-mono`} placeholder="~/projects/foo" />
            </Field>
            {spec.type === 'command' && (
              <Field label="测试命令" hint="可选，无副作用的替代命令">
                <input value={spec.testCommand ?? ''} onChange={e => patchSpec({ testCommand: e.target.value })} className={`${inputClass} font-mono`} />
              </Field>
            )}

            {approvalReason && (
              <div className="rounded-[10px] border border-amber-200 bg-amber-50 px-3 py-2 dark:border-amber-900/40 dark:bg-amber-950/20">
                <div className="flex items-start gap-2 text-[11px] text-amber-700 dark:text-amber-300">
                  <ShieldAlert size={13} className="mt-0.5 shrink-0" />
                  <span>{approvalReason}</span>
                </div>
                <button
                  onClick={() => void handleSubmit(true)}
                  disabled={submitting}
                  className="mt-2 h-6 rounded-md bg-amber-500 px-2 text-[11px] font-medium text-white hover:bg-amber-600 disabled:opacity-40 transition-colors"
                >
                  了解风险，仍然执行测试
                </button>
              </div>
            )}
            {testFailure && (
              <div className="rounded-[10px] border border-red-200 bg-red-50 px-3 py-2 dark:border-red-900/40 dark:bg-red-950/20">
                <div className="flex items-center gap-1.5 text-[11px] font-medium text-red-700 dark:text-red-300">
                  <CircleAlert size={13} /> 测试未通过，任务未创建
                </div>
                <pre className="mt-1.5 max-h-[120px] overflow-y-auto whitespace-pre-wrap break-all text-[10px] text-red-600/80 dark:text-red-300/70">{testFailure}</pre>
              </div>
            )}
            {error && (
              <div className="flex items-start gap-2 rounded-[10px] border border-red-200 bg-red-50 px-3 py-2 text-[11px] text-red-700 dark:border-red-900/40 dark:bg-red-950/20 dark:text-red-300">
                <CircleAlert size={13} className="mt-0.5 shrink-0" />
                <span>{error}</span>
              </div>
            )}

            <div className="flex gap-2 pt-1">
              <button
                onClick={() => { setStage('describe'); setError(null); setApprovalReason(null); setTestFailure(null); }}
                disabled={submitting}
                className="h-8 rounded-[10px] px-3 text-[12px] text-gray-500 dark:text-[#9CA3AF] hover:bg-gray-100 dark:hover:bg-[#1A1D24] disabled:opacity-40 transition-colors"
              >
                返回
              </button>
              <button
                onClick={() => void handleSubmit()}
                disabled={!configValid || submitting}
                className="flex h-8 flex-1 items-center justify-center gap-1.5 rounded-[10px] bg-brand text-[12px] font-medium text-white hover:bg-brand-hover disabled:opacity-40 transition-colors"
              >
                {submitting && <LoaderCircle size={13} className="animate-spin" />}
                {submitting ? '正在真实执行测试，可能需要几分钟…' : '测试并创建'}
              </button>
            </div>
          </div>
        )}
      </motion.div>
    </motion.div>
  );
}
