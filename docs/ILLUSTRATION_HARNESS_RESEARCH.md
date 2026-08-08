# 生图 Harness 调研文档：基于 PaperBanana 的 pwcli 深度集成方案

Status: Implemented foundation
Date: 2026-08-08
Scope: 调研 PaperBanana（`/Users/likuang/liki_dev/PaperBanana`）的绘图机制，论证在 pwcli 中产出"生图 harness"的设计与修改路径。

---

## 1. 背景与目标

PaperBanana 是一个参考驱动的多智能体学术绘图框架（fork 自 Google PaperVizAgent），通过
Retriever → Planner → Stylist → Visualizer → Critic 五个 agent 的流水线，把科学内容转化为
出版级插图。其能力散布在分立的任务模式（`task_name` / `exp_mode`）和独立 UI 入口中。

本 harness 的目标：

1. **统一入口**：一个入口覆盖五个场景（diagram / plot / polish / refine / eval），由 harness
   内部路由，而不是让用户/主 agent 分头调用。
2. **效果等价**：保留 PaperBanana 全部原有机制与质量水准——few-shot 检索、Planner 坐标枚举、
   Stylist 风格指南、Critic 多轮闭环（含回滚）、matplotlib 代码沙箱执行、生图模型直出、
   五维验收评测。
3. **深度集成**：不是外挂子进程服务，而是作为 pwcli 的领域管线模块 + 工具 + 内置 Skill，
   复用 pwcli 现有 LLM 客户端、生图管线、沙箱、artifact、progress、权限体系。

---

## 2. PaperBanana 调研结论

### 2.1 任务类型与渲染路线（关键分野）

| 任务 | 输入 | 渲染路线 |
|---|---|---|
| diagram（概念示意图） | Methodology 文本 + 图注 | 生图模型直出（Gemini 原生 / OpenRouter / gpt-image） |
| plot（统计图） | 原始数据（表格/JSON）+ 视觉意图 | LLM 生成 matplotlib 代码 → 隔离子进程真实执行渲染 |

路线选择是**静态配置**（入口 `--task_name`），不存在运行时自动路由，也不存在
"代码画不出就降级生图模型"的回退——plot 代码失败时走 Critic 简化描述、重新生成代码。
代码中保留了一段注释掉的实验分支，可把 plot 改为生图模型直出。

生图模型只在三处被使用：diagram 主流程、UI 的 Refine Image（编辑/超分 2K/4K）、
`dev_polish` 模式（已有图按风格指南建议重绘，强制保留数据数值）。

### 2.2 五阶段流水线机制

编排入口：`utils/paperviz_processor.py`（`process_single_query` 按 `exp_mode` 组合 agent）。

1. **Retriever**（`agents/retriever_agent.py`）
   - 用 LLM 从参考库（`data/PaperBananaBench/{task}/ref.json`）选出 Top-10 相关参考。
   - 四种模式：auto（LLM 检索）/ manual（预置 few-shot 文件）/ random / none；
     参考库缺失时优雅降级为 none。
   - 批量场景下 retriever 只跑一次，结果共享给所有候选（`process_queries_batch`）。

2. **Planner**（`agents/planner_agent.py`）
   - few-shot：每个参考示例 = 原始内容 + 视觉意图 + 参考图 base64，图文交错注入。
   - plot 系统提示要求：变量→视觉通道（x/y/hue）映射、**逐一枚举每个数据点坐标**、
     精确 HEX 色值、字号、线宽、marker 尺寸、图例位置、网格样式。
   - diagram 可选 metaphor 模式（先找视觉隐喻再展开描述）。

3. **Stylist**（`agents/stylist_agent.py`）
   - 读取 `style_guides/neurips2025_{task}_style_guide.md`，只做审美增强不改语义。
   - 风格指南按图表类型组织（柱状图黑描边/误差棒平头 cap、折线图必带 marker、
     饼图优先 Donut + 白分隔、热力图方格 + 格内数值等），并附避坑清单。

4. **Visualizer**（`agents/visualizer_agent.py`）
   - diagram：调生图模型（三供应商分支），PNG→JPG。
   - plot：prompt 模板 "Use python matplotlib to generate a statistical plot…"，
     模型返回代码 → `ProcessPoolExecutor(max_workers=32)` 子进程执行。

5. **Critic**（`agents/critic_agent.py` + `_run_critic_iterations`）
   - 看图 + 描述 + 原始数据/方法文本 → 输出 JSON（`critic_suggestions` + `revised_description`）。
   - ≤3 轮；"No changes needed." 早停并复用上一轮图。
   - 新一轮渲染失败 → **回滚到上一最佳图**并终止。
   - plot 代码执行失败时，Critic 收到 `[SYSTEM NOTICE]`，被要求给出更简化稳健的描述。

### 2.3 plot 代码执行机制（`utils/plot_execution.py`，65 行核心）

```text
extract_plot_code: 正则提取 ```python 围栏内代码
execute_plot_code_worker:
  matplotlib.use("Agg", force=True)      # 无头后端
  plt.close("all"); plt.rcdefaults()     # 干净环境
  exec(code, {})                         # 空 globals
  无 figure → None（失败）
  plt.savefig(format="jpeg", bbox_inches="tight", dpi=300) → base64
  异常捕获返回 None；finally 中关闭所有 figure
```

### 2.4 验收评测机制（`utils/eval_toolkits.py` + `prompts/*_eval_prompts.py`）

- 与人画 GT 图成对比较，LLM-as-Judge 四维度：faithfulness / conciseness / readability /
  aesthetics，每维度带一票否决红线（如 diagram 的重大幻觉、逻辑矛盾、scope 越界、乱码；
  plot 的数据失真、标签编造、错误图表类型）。
- overall 两级规则：Tier1 = faithfulness + readability，平局才进 Tier2 =
  conciseness + aesthetics（准确性永远优先于美观）。
- plot 的 faithfulness 以**原始数据为绝对 ground truth**；diagram 只能相对比较。
- 生成失败直接判 Human 胜。
- diagram 生成中验收由 Critic 闭环承担（幻觉/文字 QA/示例验证/caption 排除）。

### 2.5 图表类型如何决定

无显式分类器，四层叠加：用户 `visual_intent` 主导 → Retriever 按意图+数据形态检索同类参考 →
Planner 决定视觉通道映射 → Critic 以 "Wrong Chart Type" 红线兜底。支持范围 =
matplotlib 能画的 + 风格指南显式覆盖的类型（柱/折线/散点/饼·环/热力/雷达/treemap/lollipop/3D）。

---

## 3. pwcli 现有能力盘点（可复用积木）

| 能力 | 位置 | 复用方式 |
|---|---|---|
| 多模态 LLM 客户端（消息支持 base64 图像） | `pwcli/src/ai/llm/`（client.rs / models.rs） | planner/stylist/critic/judge 全部文本+视觉调用 |
| 生图管线（prompt 编译、image_refs、invariants、QA 记录） | `pwcli/src/runtime/image_generation.rs` + `visual_generation.rs` | diagram Visualizer、refine、polish 的渲染路径 |
| bash 沙箱执行（SandboxConfig + 超时） | `pwcli/src/runtime/bash/` | matplotlib 代码执行隔离的参考范式 |
| 工具注册/注册表/影响级别/输出 artifact | `pwcli/src/runtime/tools/register.rs`、`registry.rs`、`artifacts.rs` | 新工具挂载点 |
| Skills 系统（SKILL.md 解析 + use_skill + 内置编译 skill 先例） | `pwcli/src/runtime/skills/mod.rs`（BUILTIN_ARCHIFY_SKILL 模式） | illustration 路由 Skill |
| 架构棘轮守卫 | `scripts/check-pwcli-architecture.mjs` + `config/pwcli-architecture-allowlist.json` | 新模块需登记，只减不增例外 |
| progress / permissions / ToolExecutionContext | `pwcli/src/runtime/tools/progress.rs` 等 | 长管线进度上报与身份传递（ADR-0001 规则 6） |

架构约束（ADR-0001）：依赖方向 entry adapters → composition → agent core → AI adapters；
tools 不得依赖 service；新调用状态走 `ToolExecutionContext` 扩展。

既有先例证明"领域确定性管线作为工具"是被接受的模式：archify（typed renderer +
Skill 路由）、image_generation（prompt 编译 + QA 闭环）。

---

## 4. 生图 Harness 设计方案

### 4.1 形态结论

**Rust 确定性管线模块 + 统一入口工具 `illustrate` + 少量原子工具 + 内置 Skill 路由。**

不采用"纯 SKILL.md 驱动主 agent 即兴编排"，因为 PaperBanana 的质量来自确定性编排
（精确 prompt、回滚语义、并行候选、早停规则），主 agent 即兴编排会产生 prompt 漂移且
无法可靠实现回滚/候选并行。也不做成独立 Python 服务——那会丢失与 pwcli 生图管线、
artifact、权限的深度集成。

### 4.2 统一路由（覆盖五场景）

`illustrate` 工具参数 `mode: auto | diagram | plot | polish | refine | eval`。
`auto` 规则 + 一次轻量 LLM 分类：

```text
结构化数据（表格/JSON）          → plot
输入图 + 编辑/超分指令           → refine
输入图 + 美化/风格化意图         → polish
参考图/GT + 评审意图             → eval
其余（方法文本 + 图注）          → diagram
```

### 4.3 模块布局

```text
pwcli/src/runtime/illustration/
├── mod.rs              # IllustrationPipeline：编排 + 并行候选（tokio semaphore）
├── router.rs           # 场景路由（mode=auto 分类）
├── reference_store.rs  # 参考库（~/.pwcli/illustration/references/，可导入 PaperBananaBench）
├── retriever.rs        # LLM 检索 Top-10（auto/manual/random/none）
├── planner.rs          # planner 提示移植（含 metaphor 选项）
├── stylist.rs          # stylist 提示 + 风格指南
├── critic.rs           # critic 提示 + JSON 修复解析 + 回滚/早停/SYSTEM NOTICE
├── plot_runner.rs      # matplotlib 代码执行（python 子进程 + Agg + 超时）
└── judge.rs            # eval：四维度 + 一票否决 + Tier1/Tier2 汇总

pwcli/resources/illustration/
├── prompts/            # PaperBanana prompts 逐字节移植，include_str! 编译进二进制
└── style_guides/       # neurips2025_plot/diagram_style_guide.md 原样打包
```

依赖方向：`runtime/illustration` → `agent_core/contracts` + `ai/llm`，禁止引用 service。

### 4.4 组件映射表（效果保留依据）

| PaperBanana 组件 | pwcli 实现 | 必须保留的语义 |
|---|---|---|
| Retriever | `retriever.rs` + `reference_store.rs` | few-shot 图文交错注入；参考库缺失降级 none；批量共享检索结果 |
| Planner | `planner.rs` | 数据点坐标枚举；视觉通道映射；aesthetic 参数精确到 HEX/字号 |
| Stylist | `stylist.rs` | 只改审美不改语义；风格指南全文注入 |
| Visualizer(diagram) | 复用 `image_generation` 管线 | 免费获得 image_refs/invariants/QA，能力只增不减 |
| Visualizer(plot) | `plot_runner.rs` + `render_plot_code` 工具 | Agg 后端；plt.close/rcdefaults；空 globals；无 figure=失败；300dpi JPEG |
| Critic 闭环 | `critic.rs` | ≤3 轮；"No changes needed." 早停且复用上一轮图；失败回滚上一最佳图；SYSTEM NOTICE 简化重试 |
| 并行候选 | pipeline semaphore | retriever 只跑一次；进度经 progress 工具上报 |
| eval | `judge.rs` + `judge_illustration` 工具 | 四维度红线 + Tier 规则；生成失败判负 |
| exp_mode 矩阵 | `illustrate` 参数组合 | vanilla / full / planner_critic / polish 等价可达 |

### 4.5 工具注册（改 `runtime/tools/register.rs`）

1. `illustrate`：统一入口。入参 `{ content, visual_intent, images[], mode, num_candidates,
   max_critic_rounds, retrieval_setting, aspect_ratio, resolution }`；返回候选 artifact
   卡片 + 管线演化元数据（各阶段描述、critic 建议、plot 代码），等价 demo.py evolution timeline。
2. `render_plot_code`：原子工具，主 agent 可单独调用执行 matplotlib 代码出图。
3. `judge_illustration`：原子工具，成对比较或对照原始数据单图验收。
4. polish/refine 不新增渲染工具：管线内先跑 LLM suggestion（移植 PolishAgent 两步式），
   再走 `generate_image` 的 image_refs 编辑路径。

### 4.6 Skill 路由层（agent 行为级集成）

仿 `BUILTIN_ARCHIFY_SKILL` 内置编译 `illustration` SKILL.md，定义路由边界：

- 技术拓扑/流程/时序/状态图 → `archify`
- 学术插图、统计图、已有图美化/编辑/验收 → `illustrate`
- 通用照片/插画/营销图 → `generate_image`

用户只说"画图"，主 agent 经 `use_skill` 获得路由指南后调用统一入口——
"一个入口覆盖五场景"的最终体验由此成立。

---

## 5. 功能等价验收清单

- [ ] plot：matplotlib 代码路线，生图模型不参与、不作回退
- [ ] plot 执行失败 → SYSTEM NOTICE → 简化描述重试；渲染失败回滚上一最佳图
- [ ] 检索四模式（auto/manual/random/none）+ 缺库降级
- [ ] Planner few-shot 图文交错 + 坐标枚举约束
- [ ] Stylist 风格指南注入且语义零改动
- [ ] Critic ≤3 轮、早停、复用上一轮图
- [ ] diagram 走 pwcli 生图管线（含 QA 记录）
- [ ] refine/polish 保留数据数值不变的约束
- [ ] eval 四维度 + 一票否决 + Tier1/Tier2；失败判负
- [ ] 并行候选 + retriever 结果共享 + 进度上报
- [ ] exp_mode 等价组合可达（vanilla/full/planner_critic/polish）
- [ ] 架构检查通过，allowlist 无新增例外

---

## 6. 分期实施路线

| 阶段 | 内容 | 产出 |
|---|---|---|
| P1 | plot 路线：`plot_runner` + `render_plot_code` + planner/stylist/critic 文本管线 | 统计图全链路可用，不依赖生图模型；移植 plot 执行语义单测 |
| P2 | diagram 路线：接入 `image_generation` 作为 Visualizer | 示意图全链路可用 |
| P3 | 检索：reference_store + PaperBananaBench 种子导入 + 用户自建参考库 | few-shot 能力闭环 |
| P4 | polish / refine / eval + `mode=auto` 路由器 | 五场景统一入口完成 |
| P5 | 内置 illustration Skill + Web UI（routes/SSE）候选演化展示 | 体验层集成 |

### 6.1 当前实现决策（2026-08-08）

- 代码中的正式名称为 Runtime `IllustrationService`，不新增 Agent Harness profile。
- Agent 默认只暴露统一的 `illustrate` 工具；plot runner 与 judge 保持内部组件。
- 内置 `pwcli-starter` 精简参考库包含 96 个 pwcli 自有合成 WebP（64 diagram、32 plot），压缩包受 15 MiB 硬门禁约束；不包含 PaperBananaBench 图片、GT 或论文原图。
- 检索优先级为本次显式图片 → 用户主动导入的个人库 → 内置 Starter Pack。
- 当前 Turn 文本模型由 `ToolExecutionContext` 显式注入；生图继续复用 `tools.genImage`；视觉 Critic 可选择已配置 vision provider。
- plot 仅允许经 OS 隔离 adapter 执行，缺少可靠隔离或 matplotlib 时 fail closed，不回退生图模型。

---

## 7. 风险与待决策项

1. **matplotlib 运行环境（已落实）**：daemon 启动后非阻塞检测显式 Python、当前
   virtualenv/Conda、工作目录 `.venv` 与系统常见 Python；已有 Matplotlib ≥3.8 时直接
   复用，否则在 pwcli data directory 自动创建专用 venv 并安装 `matplotlib>=3.8,<4`。
   首次 plot 与启动任务共享同一 bootstrap，安装状态和日志持久化；系统 Python 不被修改。
2. **参考库种子**：是否随 pwcli 打包 PaperBananaBench 子集（注意数据集许可）；
   备选：纯用户自备 + 检索降级 none。
3. **默认候选数**：每候选成本 ≈ retriever+planner+stylist+visualizer+3×critic 次 LLM 调用；
   CLI 场景建议默认 3–4，上限可配。
4. **judge 模型选择**：评测维度建议与生成主模型解耦配置，避免自评偏置。
5. **生成代码安全性**：模型生成的 python 代码在沙箱外执行是高危操作，`render_plot_code`
   必须默认拒绝网络访问、限制工作目录，并纳入 permissions 审批体系。

---

## 8. 附：关键源码索引

PaperBanana（调研对象）：

- 编排：`/Users/likuang/liki_dev/PaperBanana/utils/paperviz_processor.py`
- 五 agent：`/Users/likuang/liki_dev/PaperBanana/agents/{retriever,planner,stylist,visualizer,critic,vanilla,polish}_agent.py`
- plot 执行：`/Users/likuang/liki_dev/PaperBanana/utils/plot_execution.py`
- 风格指南：`/Users/likuang/liki_dev/PaperBanana/style_guides/neurips2025_{plot,diagram}_style_guide.md`
- 评测：`/Users/likuang/liki_dev/PaperBanana/utils/eval_toolkits.py`、`prompts/{plot,diagram}_eval_prompts.py`

pwcli（集成宿主）：

- 架构 ADR：`docs/adr/0001-pwcli-four-layer-architecture.md`
- 工具注册：`pwcli/src/runtime/tools/register.rs`
- Skills：`pwcli/src/runtime/skills/mod.rs`、`pwcli/resources/skills/archify/SKILL.md`
- 生图管线：`pwcli/src/runtime/image_generation.rs`、`visual_generation.rs`
- 沙箱：`pwcli/src/runtime/bash/`
- 架构守卫：`scripts/check-pwcli-architecture.mjs`、`config/pwcli-architecture-allowlist.json`
