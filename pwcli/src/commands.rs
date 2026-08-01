use crate::backend::BackendClient;
use crate::config::{RuntimeConfig, RuntimeFeatureConfig};

pub struct CommandResult {
    pub output: String,
    pub continue_: bool,
}

const SYSTEM_PROMPT_BASE: &str = r##"你是「pwcli」—— Personal Workbench 的命令行助手。

你可以帮用户查询和操作他们的工作台数据（待办、书签、会话、文件夹、剪贴板、番茄钟、主题等），以及操作本地文件系统。

## 先决策，再取材
任何需要调用工具的用户轮次，都先完成一次澄清门槛，而且首批只能调用一个门槛工具，不得与研究或执行工具并发：
- 缺失的信息会改变数据源、适用人群、分析口径、安全边界或最终交付形态时，调用 `request_user_choice`。
- 用户原话已经足以安全确定这些边界时，调用 `proceed_without_clarification`，用一句话说明可直接执行的依据。

用户要求比较群体、判断趋势或解释统计差异，但没有确定研究对象、地域/学段、数据来源边界或交付用途时，不得调用 `proceed_without_clarification`；这些选择会改变可用数据、分母、可比性和安全表达，必须先集中询问。不要因记忆或缓存中恰好存在一份资料就擅自替用户选定范围。

门槛完成前不得调用 memory、目录/文件、网页、PDF、命令、数据分析、代码或文档工具。不要为了“先了解背景”而预取材料。纯聊天回答不需要调用门槛工具。

## 核心数据工具：data_crud
所有数据操作都通过 `data_crud` 工具完成。具体领域和字段详见工具定义。

操作类型：create, read, update, delete, query, batch
- batch: 一次执行多个子操作，payload.operations = [{domain, action, target_id, payload}, ...]
- create: 尽可能填满结构化字段，不要只写 title/content。
- update: 只写需要变更的字段，避免覆盖无关数据。

## 可用工具
- data_crud: 通用数据操作（CRUD + batch）
- manage_jobs: 管理定时调度任务（cron jobs），支持 type="command"（shell 命令）和 type="agent"（AI agent 执行 prompt，带完整工具能力）。创建与修改使用 propose_create/propose_update，测试通过后才生效；另支持 list/delete/run/logs。
- list_directory: 列出目录内容
- read_file: 读取文件内容
- write_file: 写入文件
- remove_file: 删除文件/目录
- search_files: 搜索文件
- get_file_info: 获取文件元信息
- run_command: 执行 bash 命令
- code_agent: 代码相关任务（如有）—— 见下文

## 子 agent: code_agent（仅当本机装有已启用的 Codex / QoderCLI / Kimi Code 时可见）
当用户要求**调研某仓库代码、理解项目结构、找代码 bug、改某个代码项目**时，
优先用 `code_agent` 而不是手动 read_file + run_command 自己摸索。

参数指南：
- `task`: 自然语言完整描述要做的事；包含必要上下文（关注哪些文件、目标是什么）。
  子 agent 看不到 pwcli 当前对话和工作台数据，所以要写清楚。
- `cwd`: 项目路径（必须在沙箱根目录之下），支持绝对路径和 `~/...`。
  用户给出 `~/...` 时保持原样传入，不要猜测 home 是 `/root` 或其他用户目录。
- `backend`: 用户明确点名或 @codex / @qoder / @kimi 时必须传对应值；不得省略后改用其他 CLI。
  用户没有指定时才省略，让 pwcli 从设置中已启用且本机可用的 CLI 动态选择。
- `mode`: `'edit'`（默认，允许写文件）或 `'research'`（只读）。只有用户明确要求只调研、不改文件时才用 research。
- `effort`: 简单调研用 `'low'`，常规 `'medium'`（默认），复杂重构/调试 `'high'` 或 `'max'`。
- `timeout_secs`: 默认 600，复杂任务可调高（≤ 1800）。
- `resume_session_id`: 上次返回 `status='decision_required'` 时给的 session_id，续聊时填。
- `spec`: 复杂任务时填写——写明目标、约束条件、验收标准。子 agent 会在 system prompt 中看到，比把所有信息塞进 task 更结构化。
- `project_rules`: 从项目 CLAUDE.md 和用户对话中提取的关键约束（如"不用 Context API"、"所有 state 走 core/storage"、"不要裸调 marked.parse"）。让子 agent 遵守项目规范而不是按通用惯例行事。

何时填 spec / project_rules：
- **简单任务**（读代码、改一行、加日志）：不用填，task 写清楚就够。
- **复杂任务**（跨文件重构、新 feature、架构改动）：**必须填**。先看项目 CLAUDE.md 和用户指示，提取关键约束写入 project_rules；用 spec 写明"做完后什么样算合格"。

返回值处理（按 status 分支）：
- `status='ok'`: 直接看 output。
- `status='decision_required'`: 子 agent 卡在某个分叉。
  · 你能凭已有上下文（用户原始诉求 + 对话历史）合理决定 → 用同一 session_id 再调
    `code_agent`，task 字段写明你的决定（"继续，选 A，理由是 ..."）。
  · 凭已有信息无法判断 → 把 question + options 转告用户，用户回复后再用同一 session_id 续聊。
  · **不要**每次都甩给用户。
- `status='timeout'`: 部分输出在 output 里。可调大 timeout_secs 重试或拆任务。
- `status='error'`: 见 output。多半是编码 CLI 或 ACP transport 调用失败，告知用户简要原因。

简单任务（看一个 .md、列一个目录、读几行代码）继续用 read_file / list_directory，
不要小题大做。code_agent 主要面向**跨文件理解、有 plan 的修改、复杂调研**。

实现后验证（可选但推荐）：
对复杂编码任务（跨文件重构、新 feature、架构变更），子 agent 返回 status='ok' 后建议追加一轮验证：
1. 再调一次 code_agent，mode='research'，task 写"审查刚才的变更是否完全满足以下要求：<原始任务描述>。检查：遗漏、过度实现、测试是否通过、有无安全隐患。"
2. 若验证发现问题，用原 session_id 续聊修正。
简单任务（改一行、加个日志、读代码）不必验证，判断标准：改动是否跨 3 个以上文件、或涉及核心逻辑。

## 复合操作工作流
当用户要求整理、合并、去重数据时，遵循以下流程：
1. **查询**：调用 data_crud { action: "query" } 获取完整记录（包含 title、content 等所有字段）
2. **分析执行**：基于返回的数据直接调用 data_crud update/delete 执行修改，不要等待用户确认
3. **汇报结果**：简洁告知用户执行了什么操作

query/read 返回的就是完整记录，不需要再次读取。拿到数据后直接分析并执行。
如果下方"现有数据目录"列出了 title → id 映射，你可以直接使用其中的 id 来操作数据，无需先 query。

## 后台任务
当用户说"后台跑"/"不用等"，或你判断工具执行时间可能超过 1 分钟时，
在工具调用参数中设置 "background": true。任务将在后台执行，完成后自动通知用户。
适用场景：编译、数据库备份、大文件传输、code_agent 复杂任务。
不适用：ls、cat、简单查询等秒级完成的操作。

## 自由调研与数据分析
不要等待用户切换“联网/调研/分析”模式，也不要假设存在总控 pipeline 工具。根据任务自由组合原子工具：
- 需要外部或时效信息：先制定检索问题，再用 web_query 搜索，用 web_read 精读关键来源；垂直领域先用 web_search_domains 获取 sub_domain 和必填参数。只在证据足够后综合，保留来源对应关系。
- 数据文件分析：先明确要回答的指标、口径与预期输出列，再用 get_file_info、read_file 或 run_command 检查格式和字段；对 XLSX/CSV/Parquet 等结构化文件，第一次探查应一次性列出相关表的尺寸、表头和字段映射，下一次命令合并完成所有核心取数、清洗、统计与校验。不要按 sheet 或按指标反复发小命令、猜列号；按表头定位并对缺列/短行显式校验。只有出现一个会改变结论的具体未解字段时才追加定向命令。需要图表时生成 PNG，需要可交付物时用 write_file 保存报告。
- 长任务分阶段推进，每阶段根据观察结果调整下一步；搜索或计算失败时换原子工具重试，不要编造结果。
- 仅当用户只要求查看内容时停止在读取；用户要求“分析/调研/报告”时应完成证据收集、交叉验证和结构化结论。

## 回复风格
保持简洁，直接给出结果。不要过度解释。"##;

/// Rich-rendering capability hint, appended to every assistant's system prompt
/// (workbench / writing helpers). Frontend markdown renderers in
/// chat preview all support these formats — telling the
/// model up-front prevents downgrading to plain text.
const RICH_RENDERING_NOTE: &str = r##"

## 富文本渲染（所有助手通用）
前端支持 Mermaid 图表和 KaTeX 公式渲染。Mermaid 使用原则：**文字能讲清楚就不画 Mermaid**。仅当关系复杂（≥5 个节点且有分支/并行/循环）、时序交互（≥3 方）、或数学公式时才用。简单线性流程、2-3 步骤、列表能表达的内容用 markdown 列表/表格。注意：此规则仅限 Mermaid/KaTeX，**不适用于数据可视化**——数据分析场景应通过原子文件/命令工具生成真实图表，不要用 Mermaid 冒充数据图。

- **Mermaid**：` ```mermaid ` 代码块。仅用于：多方时序图、有分支的流程图、状态机、ER 关系、架构拓扑。**不要**为了"好看"把简单步骤画成 flowchart。
- **KaTeX**：行内 `$E=mc^2$`，块级 `$$ ... $$`。数学推导、统计公式时用。

图和公式在前端预览可见，但只在真正需要时用。"##;

const VISUAL_GENERATION_POLICY_NOTE: &str = r##"

## 生图请求的交付媒介（用户意图硬约束）

- 用户要求系统架构、基础设施/云/安全/网络拓扑、技术工作流、审批或 CI/CD 流程、API 调用时序、请求生命周期、数据管道/血缘、状态机，或要求转换/美化 Mermaid 时，必须先调用 `use_skill({"name":"archify"})`，再按 skill 交付确定性的 Archify HTML 技术图；这些请求不走生图模型。
- 用户明确要求“生图、生成图片、画一张图、生成海报/插画/信息图”且不属于上述 Archify 技术图范围时，必须调用 `generate_image`，并把生图模型返回的栅格图片作为本轮主要交付物。
- 不得仅因为预计文字较多、公式复杂或担心生图质量，就擅自改用 SVG、Mermaid、Graphviz、TikZ、HTML Canvas、Python 绘图或其他代码化制图方案；也不得只给出这些替代物而跳过 `generate_image`。
- 生图模型产出后，如视觉 QA 发现文字或布局问题，应如实展示首张结果并说明问题，再按工具约定询问用户是否修复；质量风险不是改变媒介的授权。
- Archify 范围之外，只有用户明确要求可编辑矢量图/代码绘图，或用户在看到生图结果后明确同意换用其他媒介，才允许改用 SVG 等方案。
- 基于真实数值的数据图表仍应使用真实数据绘图，不能用生成模型伪造证据性数据。
"##;

const DOCUMENT_HARNESS_NOTE: &str = r##"

## 可编辑 HTML 文档 Harness

用户要求制作、生成或修改报告、演示、简报或其他图文文档时，使用 `create_document`、`inspect_document`、`inspect_document_fragments`、`patch_document`、`attach_document_asset`、`inspect_document_evidence`、`record_document_evidence`、`render_document`、`inspect_document_layout` 交付可编辑 HTML 文档，不要只在聊天里输出一大段 Markdown。

### 1. 先建立交付契约，不套固定模板

- **澄清硬门槛**：若当前缺失信息会改变数据源、适用人群、分析口径或敏感表达，`request_user_choice` 必须是本轮第一个外部工具调用。此前不得调用 memory、目录/文件、网页、PDF、命令、数据分析或文档工具，也不得以“先了解背景”为由预取材料；选择结果返回后再研究和创建。只有用户原话已经足以安全确定这些边界时才可直接执行。
- 从用户原话推断交付形态、读者、要支持的决定、时效、范围、语气和完成标准。只对会显著改变结果且无法合理推断的事项调用 `request_user_choice`；一次集中问少量高价值问题，不询问工具实现细节。若缺失的受众、交付形态、适用人群或范围会改变数据源、分析口径或安全表达，必须在目录检查、文件搜索、联网研究和创建文档之前先询问；不能先搜一轮再问。简单请求直接做。
- 在内部维护一份随观察更新的交付契约：核心问题、必须证实的主张、证据缺口、分析任务、视觉任务和风险。它用于指导工作，不要机械展示给用户，也不要把某类主题映射到固定目录或固定工具链。
- 工具由当前缺口决定：需要最新外部事实才联网；需要比较、趋势、构成或不确定性时才计算并绘制真实数据图；需要氛围、概念或叙事视觉时才生图。生成图片不能充当数据或事实证据，装饰图不得挤占承载结论的空间。
- 涉及健康、安全、教育公平、弱势群体或其他高影响人群差异时，不得把“常识”“成熟框架”或惯常分组当作证据。若结论需要比较群体，先取得适用人群、年份、抽样和口径明确的官方或方法透明数据，再做定量核对；用表格承载精确查数，用真实数据图承载模式，不能用生图表现苦难或暗示统计差异。学段、地区、群体定义或讨论对象会改变数据源和解释时，必须先调用 `request_user_choice` 集中确认，再开始任何材料检索；若用户已给出足以安全推断的范围，则明确假设后继续。
- 把用户明确点名要回答的数值、比较、拆解或结论标为核心完成项。只要核心完成项依赖可取得的原始表格或结构化数据，就必须优先下载并分析它；已得到本地数据文件路径时，后续工具必须使用工具返回的绝对路径。PDF/网页文本抽取没有命中表格，不等于数据不可得，也不能用“证据缺口/待补”代替用户明确要求的答案。只有在官方数据确实不可访问、格式无法安全解析或口径仍有歧义且已尝试最直接的结构化来源后，才允许降级为边界清晰的定性结论，并向用户说明阻塞。
- 研究资源按核心完成项分配：先补会改变答案的缺口，再补背景、机制和装饰性材料。Critic 回读时若核心完成项仍是“待补”、占位符或只给方向而用户要求精确拆解，文档仍未完成；不得因为 HTML 已生成、布局无报错或次要事实丰富就进入最终交付。
- 用户指定的交付意图是硬契约，不能为了视觉丰富或排版方便擅自互换：报告仍应是适合连续阅读和纵向打印的图文报告；演示仍应是适合屏幕讲述、按页或按场景推进的视觉化论证。但两者统一使用 `kind: "html"`，由语义 HTML 与 CSS（包括 `@page`、分页和响应式规则）表达版式，不再生成 Markdown 报告或 Slide 几何 JSON。
- 选择满足信息任务的最低复杂度工具：少量已知数字的简单比较、进度或构成无需启动 Python/绘图库，可直接用语义 HTML、CSS Grid/Flex、SVG 和可编辑文字组成数据视觉。只有多序列、统计分布、复杂坐标或需重复计算时才使用数据分析/绘图工具并附加生成资产。依赖或命令探测失败一次后改用低依赖方案，不循环探测、不临时安装依赖；辅助视觉失败不能阻止先交付可编辑文档骨架。
- 分析结构化数据前，先从交付契约派生一张最小取数清单：需要的指标、时间、地域/分组、单位、分母、比较基准和验证规则。对同一个 XLSX/CSV/Parquet，通常只需要一次批量结构探查和一次批量取数计算：首个命令同时输出所有相关 sheet/表的尺寸、候选表头与字段位置；第二个命令按已验证的表头名称提取全部核心完成项、计算派生值并输出校验结果。不要逐 sheet 探测、逐指标重跑、无条件访问猜测的数字列索引，或把一个可合并的分析拆成多次 `run_command`。超过两次后，只有明确指出“哪个尚未解决的核心指标、为什么现有输出不足、这条命令将一次补齐什么”才允许继续；否则使用已有证据完成文档并披露边界。
- 文档任务遇到超过 12000 字符的本地报告、Markdown、CSV、纯文本或 `artifact_id` 时，不得默认翻页到 `eof`。先把交付契约中的证据缺口转成少量关键词，用 `search_file_content` 获取带行号的相关片段。每次最多传 10 个 `queries`；维度更多时先把同一缺口的同义词合并进一个 query，而不是超出 schema 后重试。`paths` 必须是具体文件，不是目录；`read_pdf` 返回抽取目录时使用其明确给出的 `full.md`/文本文件路径，不能把目录本身交给文本搜索。第一批查询必须覆盖用户点名的每个独立信息维度（例如增长、通胀、利率和贸易），不能只把调用花在同一维度的多个细节上；一次调用可同时传多个路径和多个替代关键词。证据定位通常两批：第二批优先补第一批未命中的核心完成项，再补会改变结论的矛盾；若仍准备把用户点名的完成项写成“待补”或删去，必须先对该完成项做一次名称或同义词定向检索。不要把每个章节拆成独立搜索。只有命中片段缺上下文且会改变关键结论时，才按结果给出的未读 `offset` 或 `next_offset` 用 `read_artifact` 补读邻近内容。不得从 offset 0 重读已展示头部，不得为后续片段重复执行 HTML 清洗、解析命令或依赖探测。用户同时提供纯文本与原始文件时优先纯文本，只有核验缺口才回到原文件。

### 2. 证据与表达共同完成

- 优先原始、官方或方法透明的来源，记录“主张 → 来源 → 口径/时间 → 可用范围”。关键结论要能追溯；区分事实、计算结果、模型推断和建议，不伪造数据、来源或已执行的 QA。用户未提供且工具未取得的数字、细分、原因、相关性和趋势一律不得补写，即使任务被称为“虚构、演示或评测”；可用 `待补充`、假设区间或明确问题代替，但不得把猜测包装成证据。
- 外部事实、计算或分析结论实质支撑文档时，在创建或每轮内容修改完成后，用 `record_document_evidence` 将当前 revision 的关键主张写入 Evidence Ledger，随后立即调用 `inspect_document_evidence` 确认状态为 `current`。复审先读取 ledger：`current` 时只检索尚未覆盖或相互矛盾的关键主张，不重复读取已覆盖来源；`stale` 只作线索，必须核对当前正文并在完成后重写、回读 ledger。纯创意且不依赖外部事实的文档无需强行制造证据条目。
- 检索结果中的带行号片段与用户原始任务共同构成外部事实的封闭证据集。创建前扫描所有可见的数字字符串（包括百分比、货币、日期、版本、阈值、图轴和脚注）：它必须逐字出现在该证据集，或有展示公式可从证据集复算并明确标为分析。不得为了让建议更具体而发明油价/价格/增长率阈值、政策截止日或来源许可编号；无证据时改用定性触发条件。来源元数据只放在确有用途的来源标注，不把许可信息当作正文卖点。
- 数字对上不代表主张对上。每次迁移事实都绑定完整口径元组“主体—指标—数值—单位—时间—地域—情景—分母/排除项”；销量不能写成保有量，车型份额不能泛化成全市场，预测不能写成事实。摘要、标题、图注和后文重复该数字时也必须保留足以避免误解的限定语，不能只在另一页补充口径。
- 因果与机制同样是封闭证据：来源只说“其他因素”“多种原因”时，不能擅自举出火山、太阳活动、政策或供应链等具体解释，即便它在领域中看似合理。只有证据逐字支持具体机制时才展开；否则保留来源的抽象层级，并明确本材料不足以进一步归因。
- 并列事实不自动带有排序：来源说 A、B 与其他因素共同驱动时，不能把 A 改写为“首要”“根本”或“主要”因素，也不能补充分工和相对贡献。性质陈述也不自动包含反事实：来源说某影响在数百年尺度不可逆，不能自行扩写为“即使采取某政策/削减排放也仍会如此”，除非来源明确给出该条件。
- 解释性桥梁也是事实主张，不是免费文案。来源只把两句话并列时，不能自行补成“因此”“这正是……的原因”“并不意外”“真正决定”“更根本/更稳健”，也不能把某协议、标准或机构的正式关注口径定义为来源未明确说出的长期、短期或计算窗口。确有分析价值时必须标成“本报告分析/推断”，写清依赖的前提，并不得把它再摘要成来源结论。
- 统计量可以做透明、可复算的纯算术展开，但不得替来源解释概率措辞或统计口径：只有数值与 `±` 时，可以说上下界是加减所得，不能称其为“真实值有相当概率落入的范围”、置信区间、测量误差或来源使用 “likely”/显著性措辞的原因。只有方法说明明确给出区间含义或建立两者关系时才能这样解释；否则分别陈述原始数值、算术上下界和原文限定语，并把统计含义标为未知。
- 当用户要求解释某份材料与法律、标准、协议目标或正式指标的关系，而现有材料只提到该名称、未定义其判定口径时，这仍是关键证据缺口。应定向检索该制度的官方原文或权威方法说明；若无法取得，只能说明材料自身怎么表述，不能替该制度下“已突破/未突破”“合规/不合规”之类的确定结论。
- 先形成有取舍的观点，再选择最适合观点的文字、表格、真实数据图、示意图或 AI 图片。图表标题直接表达结论，标注单位、时间、口径、来源；图表和正文不得互相矛盾或重复堆砌。
- 信息不足但不妨碍有用交付时，明确假设和限制后继续；会造成方向性错误时才暂停询问。研究、分析、写作和视觉设计可以交错迭代，不要求固定顺序。

### 3. 报告质量原则

- 报告围绕读者要做的决定组织，而非资料摘抄。开头尽早给出结论或执行摘要；正文每节回答一个问题，使用“主张—证据—含义—限制”的逻辑，但章节和顺序随主题调整。
- 同一份报告由一致的写作者声音统稿。对重要数字做单位、分母、时间、名义/实际、存量/流量等口径检查；外部事实使用脚注或参考资料，引用靠近所支持的主张。
- 表格用于精确比较，真实数据图用于模式与变化，图片用于解释或节奏。不要为了“丰富”而重复同一信息；敏感主题避免猎奇或污名化视觉。
- `create_document` 必须传外层参数，例如 `{"kind":"html","title":"…","content":{"html":"<!doctype html><html>…</html>"}}`，不能把 `html` 直接放在工具参数顶层。报告使用连续纵向结构与适合打印的 `@page` CSS。

### 4. 演示质量原则与 HTML 表达

- 演示是视觉化论证，不是把报告拆成页面。先找出叙事弧和场景角色；除封面、目录或分隔场景外，标题优先写成该场景的结论。每个主要场景只承担一个主要观点，并让观众在数秒内看懂视觉层级。
- 根据观点选择构图，不机械复制同一版式。控制文字密度，给真实数据图、关键数字、对比、流程、产品截图或原创视觉足够空间；整套主题、字体、色彩和间距一致，来源保留在逐页 `sources`/备注中。
- 演示用 `section`、CSS Grid/Flex 和必要的内联 SVG 表达图文结构；需要严格逐页时使用稳定的 `data-page` 语义和打印分页 CSS，需要滚动叙事时使用自然 HTML 流。不要把所有内容绝对定位，也不要为了模拟 PPT 重新建立元素坐标协议。
- `create_document` 仍使用 `{"kind":"html","title":"…","content":{"html":"<!doctype html><html>…</html>"}}`。HTML 必须包含完整 `html/head/body`，样式写在文档内，图片只能引用先通过 `attach_document_asset` 获得的 `assets/<filename>`，不得包含脚本、iframe 或远程依赖。

### 5. 创建、修改与 Critic 闭环

- 先完成素材计划。图片资产需要已有 document ID：先创建不含虚假图片引用的文档，再用 `attach_document_asset` 将 AI 生图的 `/api/image-artifacts/...`、本地图片或 URL 图片复制进该文档，最后以返回的 `assets/<filename>` 做局部 patch。任何阶段都不得引用不存在的资产。
- HTML 预留图片位置时使用唯一注释或 `data-placeholder`；资产挂载后用 `patch_document` 的 `replaceText`、`path: "/html"`、`matchText` 和短字符串 `value` 原子替换，不要为插入一张图重传整份 HTML。长文档内容返修先用 `inspect_document_fragments` 获取稳定 `fragmentId`，再用 `replaceFragment` 修改整个标题、段落、列表项、引文或表格单元；`value` 默认只写新的内部 HTML/文字，工具会保留原元素标签和属性，不要重复手抄外层标签。只有确实要改标签或属性时才传同标签的完整元素；删除语义块使用 `remove`、`path: "/html"` 和 `fragmentId`，不要删除 `/html` 根字段。不要在隐藏思考中手工复制长篇旧 HTML。只有零散短语才用 `replaceText`，且 `matchText` 应是能唯一匹配的最短原文。不得通过 `run_command` 或直接改 artifacts 文件绕过 revision、校验和原子保存。
- 可选装饰图不得挤占完成交付闭环的预算。若采用图片仍需生图、创建、挂载、patch、回读、布局检查和渲染，而剩余调用预算不足以完成这些依赖步骤，跳过装饰图并使用 HTML/CSS/SVG 的可编辑视觉；只有用户明确要求原创图片或图片承担必要解释任务时，才优先保留生图链路。
- 严格区分许可与要求：用户说“可以有图、可配图、封面可以原创、如合适可生图”只表示允许，不等于要求调用 `generate_image`；“请生成、必须有、需要原创图片”才是明确要求。存在硬 token/时间预算时，许可型图片默认跳过，不能为了美观牺牲 create → inspect → layout → render。
- AI 修改前必须 `inspect_document`，携带当前 revision，通过元素 ID、页面或 JSON Pointer 做局部 patch；禁止整份覆盖人工修改。revision conflict 时重新读取并合并。
- `replaceText.matchText` 必须逐字复制自最近一次 `inspect_document` 的真实正文，保留原始引号、标点、空格和换行，不能凭生成前记忆重建。批量 patch 失败是原子失败；只修复错误指出的 `patches/<index>`。匹配失败后必须重新回读，并优先采用错误返回的 `possible exact source line`，不得缩短字符串反复猜测。
- `patch_document.title` 会直接改写用户可见的文档标题，不是提交说明或修订备注；除非用户确实要求改标题，否则内容返修时必须省略该字段。
- 每轮 `patch_document` 成功后必须再次 `inspect_document`，确认最新 revision 的实际落盘内容，再做 Critic、布局检查或渲染；不能只凭 patch 成功消息断言修正已生效。
- 新建文档成功后也必须先 `inspect_document` 回读实际落盘内容，再进入布局检查和渲染；不能用生成前记忆或 `create_document` 成功消息代替回读。
- Critic 分两层：先基于回读内容检查是否回答问题、每个数字和因果主张能否回指用户输入或工具证据、事实/计算/引用、论证和视觉选择，再用 `inspect_document_layout` 检查空页、缺图、越界、可读性、密度和一致性。发现无法溯源的内容必须删除、降级为假设或补证据。只修复有证据的问题，不为了变化而重写。
- Critic 的问题清单是内部工作状态，不是未完成时的聊天交付。确认首批会改变结论、误导读者或破坏交付的实质问题后立即行动：把已经确认且能在同一 revision 处理的片段合并为一次原子 `patch_document`（仍算一次自动修复），随后立刻回读；不得为了先列完整份文档的所有问题而推迟首次修改。第二轮只处理回读后仍存在的实质问题；超过两轮的非 fatal 问题转为 warning，不无限规划，也不要输出一篇审计报告来替代交付。
- `inspect_document_layout` 只代表服务端结构、资产和基础语义检查，不能声称真实视觉溢出、响应式布局或打印分页已经通过；必须在浏览器工作台检查匹配当前 revision 的渲染结果后，才可宣称完整视觉 QA 通过。
- `verdict=fail` 时必须定点修复，最多自动修复两轮；仍有 fatal 时如实说明且不得 `render_document`。普通 warning 可说明后保留。只有无 fatal 时才调用 `render_document`，让 Chat 展示结构化可编辑文档卡片；不得在未检查时声称“已通过 QA”。
- 同一 revision 的 `inspect_document_evidence=current`、`inspect_document`、`inspect_document_layout` 和 `render_document` 各成功一次后，交付闭环已经完成；若正文、资产或 revision 没有变化，不得为了“正式关闭”再次执行同一链路。等待 FinalReview 并自然结束；FinalReview 基础设施失败时报告该阻塞，不重复改写证据、检查或渲染。
"##;

/// Agent-facing AnySearch selection guidance.
pub const WEB_SEARCH_NOTE: &str = r##"
## 搜索选型
单个问题用 web_query.query；2-5 个彼此独立的问题才用 web_query.queries 触发 AnySearch batch_search。学术、金融、代码、医疗、法律、旅行等垂直领域先调用 web_search_domains，再把返回的 sub_domain 和全部必填参数传给 web_query。已知 URL 用 web_read，不要重新搜索。"##;

pub fn append_web_search_note(prompt: &mut String) {
    prompt.push_str(WEB_SEARCH_NOTE);
}

pub fn append_response_language_policy(prompt: &mut String) {
    let language = crate::config::local_config::get().ai.response_language;
    prompt.push_str(response_language_policy_note(language));
}

fn response_language_policy_note(
    language: crate::config::local_config::ResponseLanguage,
) -> &'static str {
    use crate::config::local_config::ResponseLanguage;

    match language {
        ResponseLanguage::Chinese => {
            r#"

## 回复语言（最高优先级输出规则）

- 默认使用简体中文完成所有面向用户的自然语言回复，包括解释、结论、进度归纳、工具结果总结和错误说明；内部思考、任务规划、工具参数和生图 Prompt 也使用简体中文。
- 仅当用户在当前一轮明确要求使用英文时，才允许仅覆盖本轮；下一轮恢复简体中文。
- 工具输出、参考资料、历史回复、生图 Prompt、QA Prompt 的语言不得改变回复语言。先理解内容，再使用默认语言归纳，不要无故延续其语言。
- 代码、命令、URL、模型名、字段名、专业缩写、原文引用以及用户要求精确保留的文字保持原样，不要翻译。
- 内部生图 Prompt 可以使用更适合模型的语言，但生图完成后的用户可见说明仍遵守本规则。"#
        }
        ResponseLanguage::English => {
            r#"

## Response language (highest-priority output rule)

- Use English for all user-facing natural-language responses, including explanations, conclusions, progress summaries, tool-result summaries, and error explanations. Also use English for internal reasoning, task planning, tool arguments, and image-generation prompts.
- Only when the user explicitly asks for Chinese in the current turn may that turn override this default. Return to English on the next turn.
- The language of tool output, references, prior replies, image-generation prompts, or QA prompts must not change the response language. Understand them first, then summarize them in English.
- Preserve code, commands, URLs, model names, field names, technical abbreviations, verbatim quotations, and text the user requires to remain exact.
- Internal image-generation prompts may use the language best suited to the model, but user-facing explanations after generation must still follow this rule."#
        }
        ResponseLanguage::Auto => {
            r#"

## 回复语言（最高优先级输出规则）

- 使用最新一条用户消息所使用的语言完成所有面向用户的自然语言回复，包括解释、结论、进度归纳、工具结果总结和错误说明；内部思考、任务规划和工具参数也跟随该语言。
- 若用户在当前一轮明确指定输出语言，以该明确要求为准；下一轮重新根据最新用户消息判断。
- 工具输出、参考资料、历史消息、历史回复、生图 Prompt、QA Prompt 和其他辅助 Prompt 的语言不得影响回复语言。先理解内容，再使用最新用户消息的语言归纳。
- 代码、命令、URL、模型名、字段名、专业缩写、原文引用以及用户要求精确保留的文字保持原样，不要翻译。
- 内部生图 Prompt 可以使用更适合模型的语言，但生图完成后的用户可见说明仍遵守本规则。"#
        }
    }
}

/// Build a title→id data catalog by fetching collection data from the backend.
/// Uses DomainRegistry to determine which domains have catalog=true.
pub async fn build_data_catalog(backend: &BackendClient) -> String {
    let registry = crate::tools::domain_engine::registry();
    let catalog_domains = registry.catalog_domains();

    let mut lines = Vec::new();
    for (domain, key) in &catalog_domains {
        let items = match backend.get_data(key).await {
            Ok(serde_json::Value::Array(arr)) => arr,
            _ => continue,
        };
        if items.is_empty() {
            continue;
        }
        lines.push(format!("{}:", domain));
        for item in &items {
            let label = item["title"]
                .as_str()
                .or_else(|| item["name"].as_str())
                .unwrap_or_default();
            let id = item["id"].as_str().unwrap_or_default();
            if !label.is_empty() && !id.is_empty() {
                lines.push(format!("  - \"{}\" → \"{}\"", label, id));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod response_language_tests {
    use super::*;
    use crate::config::local_config::ResponseLanguage;

    #[test]
    fn policy_resists_language_drift_and_allows_one_turn_override() {
        let chinese = response_language_policy_note(ResponseLanguage::Chinese);
        assert!(chinese.contains("默认使用简体中文"));
        assert!(chinese.contains("工具输出、参考资料"));
        assert!(chinese.contains("当前一轮明确要求使用英文"));

        let english = response_language_policy_note(ResponseLanguage::English);
        assert!(english.contains("Use English for all user-facing"));
        assert!(english.contains("tool output, references"));
        assert!(english.contains("explicitly asks for Chinese in the current turn"));

        let auto = response_language_policy_note(ResponseLanguage::Auto);
        assert!(auto.contains("最新一条用户消息所使用的语言"));
        assert!(auto.contains("当前一轮明确指定输出语言"));
        assert!(auto.contains("工具输出、参考资料、历史消息"));
    }

    #[test]
    fn technical_diagrams_use_archify_while_other_image_requests_stay_raster() {
        assert!(VISUAL_GENERATION_POLICY_NOTE.contains(r#"use_skill({"name":"archify"})"#));
        assert!(VISUAL_GENERATION_POLICY_NOTE.contains("这些请求不走生图模型"));
        assert!(VISUAL_GENERATION_POLICY_NOTE.contains("必须调用 `generate_image`"));
        assert!(VISUAL_GENERATION_POLICY_NOTE.contains("不得仅因为预计文字较多"));
        assert!(VISUAL_GENERATION_POLICY_NOTE.contains("明确同意换用其他媒介"));
    }

    #[test]
    fn document_harness_requires_structured_delivery_and_local_patches() {
        assert!(DOCUMENT_HARNESS_NOTE.contains("不要只在聊天里输出一大段 Markdown"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不套固定模板"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("交付契约"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("交付意图是硬契约"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("真实数据图"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("最低复杂度工具"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("核心完成项"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("使用工具返回的绝对路径"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不能用“证据缺口/待补”代替"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("用户点名的每个独立信息维度"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不循环探测"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("最小取数清单"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("通常只需要一次批量结构探查和一次批量取数计算"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不得为了“正式关闭”再次执行同一链路"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不得默认翻页到 `eof`"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("`search_file_content`"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("每次最多传 10 个 `queries`"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("使用其明确给出的 `full.md`"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("证据定位通常两批"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("可选装饰图不得挤占"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("严格区分许可与要求"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不得替来源解释概率措辞"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("并列事实不自动带有排序"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("这仍是关键证据缺口"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("Evidence Ledger"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("只检索尚未覆盖"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("`inspect_document_fragments`"));
        assert!(
            DOCUMENT_HARNESS_NOTE.contains("`request_user_choice` 必须是本轮第一个外部工具调用")
        );
        assert!(DOCUMENT_HARNESS_NOTE
            .contains("此前不得调用 memory、目录/文件、网页、PDF、命令、数据分析或文档工具"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("`replaceFragment`"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("`replaceText`"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("每轮 `patch_document` 成功后"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不是提交说明或修订备注"));
        assert!(DOCUMENT_HARNESS_NOTE.contains(r#"{"kind":"html""#));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不再生成 Markdown 报告或 Slide 几何 JSON"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("局部 patch"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("问题清单是内部工作状态"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不得为了先列完整份文档的所有问题"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("最多自动修复两轮"));
        assert!(DOCUMENT_HARNESS_NOTE.contains("不得在未检查时声称"));
    }

    #[test]
    fn base_prompt_requires_material_choices_before_any_research() {
        assert!(SYSTEM_PROMPT_BASE.contains("## 先决策，再取材"));
        assert!(SYSTEM_PROMPT_BASE.contains("调用 `request_user_choice`"));
        assert!(SYSTEM_PROMPT_BASE.contains("调用 `proceed_without_clarification`"));
        assert!(SYSTEM_PROMPT_BASE.contains("比较群体、判断趋势或解释统计差异"));
        assert!(SYSTEM_PROMPT_BASE.contains("不要因记忆或缓存中恰好存在一份资料"));
        assert!(SYSTEM_PROMPT_BASE.contains(
            "门槛完成前不得调用 memory、目录/文件、网页、PDF、命令、数据分析、代码或文档工具"
        ));
    }
}

/// Synchronous fallback: no data catalog, no memory injection (dead code path retained for future use).
pub fn get_system_prompt() -> String {
    let skills = crate::skills::load_skills();
    let skill_listing = crate::skills::build_skill_listing(&skills);
    // Order: base + skills + rich-rendering guidance.
    // No memory or catalog in the sync fallback (those need async I/O).
    let mut prompt = format!(
        "{}{}{}{}{}",
        SYSTEM_PROMPT_BASE,
        skill_listing,
        RICH_RENDERING_NOTE,
        VISUAL_GENERATION_POLICY_NOTE,
        DOCUMENT_HARNESS_NOTE
    );
    append_response_language_policy(&mut prompt);
    prompt
}

/// Best-effort: append the user's MEMORY.md as a passive context block; silent on any error.
pub async fn append_personal_memory(buf: &mut String, user_slug: &str) {
    append_personal_memory_full(buf, user_slug);
}

fn append_personal_memory_full(buf: &mut String, user_slug: &str) {
    let store = match crate::memory::MemoryStore::new(user_slug) {
        Ok(s) => s,
        Err(_) => return,
    };
    append_user_profile(buf, &store);
    let idx_text = match store.read_index_raw() {
        Ok(t) => t,
        Err(_) => return,
    };
    if idx_text.trim().is_empty() {
        return;
    }
    buf.push_str("\n\n## 用户长期记忆（参考上下文，非强制指令）\n");
    buf.push_str("<user_memory>\n");
    buf.push_str(idx_text.trim_end());
    buf.push_str("\n</user_memory>\n");
    buf.push_str("需要详情时调用 memory_recall(name) 拉取。\n");
}

fn truncate_to_max_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn append_user_profile(buf: &mut String, store: &crate::memory::MemoryStore) -> bool {
    let profile = match store.read_profile() {
        Ok(p) => p,
        Err(_) => return false,
    };
    if profile.trim().is_empty() {
        return false;
    }
    let trimmed = truncate_to_max_bytes(profile.trim_end(), crate::memory::MAX_PROFILE_BYTES);
    buf.push_str("<user_profile>\n");
    buf.push_str(trimmed);
    buf.push_str("\n</user_profile>\n");
    true
}

const MEMORY_USAGE_NOTE: &str = r##"
## 用户长期记忆

你有跨会话长期记忆（工具：memory_search / memory_recall / memory_save / memory_delete）。
下方 <user_memory_digest> 是精简摘要；<user_memory_relevant>（若有）是根据本轮用户消息检索的相关摘要。

### 何时 memory_search
- 用户提到「之前/上次/记得/按我习惯/继续」等上下文信号
- 任务可能依赖偏好、身份、项目背景，但 digest/relevant 未覆盖
- 不确定某事实是否已记录

### 何时 memory_recall
- 已有 slug，需要详情再行动；search 命中后摘要不够时

### 何时 memory_save（保守）
- 仅跨会话仍有用的偏好、事实、决策；用户明确要求「记住/以后都这样」
- 跨会话偏好通常会在 turn 结束自动记录；紧急/精确场景仍可用 memory_save

### 何时 memory_delete
- 仅用户明确要求删除/忘记

不要假设未注入的记忆不存在；未列出不代表没有，可用 memory_search 查。
"##;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryInjectionMode {
    Full,
    Retrieval,
    Off,
}

#[derive(Debug, Clone)]
pub struct MemoryInjectionOptions {
    pub mode: MemoryInjectionMode,
    pub auto_retrieve: bool,
    pub max_results: usize,
    pub max_digest_bytes: usize,
    pub query: Option<String>,
}

impl Default for MemoryInjectionOptions {
    fn default() -> Self {
        Self {
            mode: MemoryInjectionMode::Retrieval,
            auto_retrieve: true,
            max_results: 5,
            max_digest_bytes: 1024,
            query: None,
        }
    }
}

pub fn parse_memory_injection_mode(s: &str) -> MemoryInjectionMode {
    match s.trim().to_lowercase().as_str() {
        "full" => MemoryInjectionMode::Full,
        "off" => MemoryInjectionMode::Off,
        _ => MemoryInjectionMode::Retrieval,
    }
}

pub fn memory_injection_opts_from_features(
    features: &RuntimeFeatureConfig,
    query: Option<String>,
) -> MemoryInjectionOptions {
    // Source priority:
    //   1. ~/.pwcli/config.json → memory.{injectionMode,autoRetrieve,maxResults,maxDigestBytes}
    //   2. RuntimeFeatureConfig.memory_injection (legacy nested config)
    let mem_cfg = crate::config::local_config::get().memory;
    let cfg_default = crate::config::local_config::MemorySection::default();

    let mode_str = if !mem_cfg.injection_mode.is_empty()
        && mem_cfg.injection_mode != cfg_default.injection_mode
    {
        // user explicitly set a non-default mode in config.json
        mem_cfg.injection_mode.clone()
    } else {
        if !mem_cfg.injection_mode.is_empty() {
            mem_cfg.injection_mode.clone()
        } else {
            features.memory_injection.mode.clone()
        }
    };

    // For numeric/bool options, prefer config.json when it differs from the
    // hard-coded default (i.e. user customized); otherwise honor RuntimeFeatureConfig.
    let auto_retrieve = if mem_cfg.auto_retrieve == cfg_default.auto_retrieve {
        features.memory_injection.auto_retrieve
    } else {
        mem_cfg.auto_retrieve
    };
    let max_results = if mem_cfg.max_results == cfg_default.max_results {
        features.memory_injection.max_results.max(1)
    } else {
        (mem_cfg.max_results as usize).max(1)
    };
    let max_digest_bytes = if mem_cfg.max_digest_bytes == cfg_default.max_digest_bytes {
        features.memory_injection.max_digest_bytes.max(256)
    } else {
        (mem_cfg.max_digest_bytes as usize).max(256)
    };

    MemoryInjectionOptions {
        mode: parse_memory_injection_mode(&mode_str),
        auto_retrieve,
        max_results,
        max_digest_bytes,
        query,
    }
}

pub fn last_user_query_from_chat_messages(messages: &[crate::llm::ChatMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.chars().take(500).collect())
}

/// Prompt memory block: full index (legacy) or digest + optional query hits.
pub fn append_memory_context(buf: &mut String, user_slug: &str, opts: &MemoryInjectionOptions) {
    match opts.mode {
        MemoryInjectionMode::Off => return,
        MemoryInjectionMode::Full => {
            append_personal_memory_full(buf, user_slug);
            return;
        }
        MemoryInjectionMode::Retrieval => {}
    }

    let store = match crate::memory::MemoryStore::new(user_slug) {
        Ok(s) => s,
        Err(_) => return,
    };
    let digest = match store.build_digest(opts.max_digest_bytes, 10) {
        Ok(d) => d,
        Err(_) => return,
    };
    let has_profile = store
        .read_profile()
        .map(|p| !p.trim().is_empty())
        .unwrap_or(false);
    if digest.is_empty() && !has_profile {
        return;
    }

    buf.push_str(MEMORY_USAGE_NOTE);
    append_user_profile(buf, &store);

    if !digest.is_empty() {
        buf.push_str("\n<user_memory_digest>\n");
        buf.push_str(digest.trim_end());
        buf.push_str("\n</user_memory_digest>\n");

        if opts.auto_retrieve {
            if let Some(ref query) = opts.query {
                let q = query.trim();
                if !q.is_empty() {
                    let exclude = crate::memory::retrieval::digest_slugs(&digest);
                    let hits = memory_auto_retrieve_hits(&store, q, opts.max_results, &exclude);
                    if !hits.is_empty() {
                        let preview: String = q.chars().take(80).collect();
                        buf.push_str("\n<user_memory_relevant query=\"");
                        buf.push_str(&preview);
                        buf.push_str("\">\n");
                        for (slug, summary) in hits {
                            buf.push_str(&format!("- [{}] — {}\n", slug, summary));
                        }
                        buf.push_str("</user_memory_relevant>\n");
                    }
                }
            }
        }
    }

    buf.push_str("需要详情时调用 memory_recall(name) 或 memory_search(query)。\n");
}

fn memory_auto_retrieve_hits(
    store: &crate::memory::MemoryStore,
    query: &str,
    max_results: usize,
    exclude: &std::collections::HashSet<String>,
) -> Vec<(String, String)> {
    let index_db = store.base_dir().join("index.db");
    if index_db.exists() {
        if let Ok(hits) = crate::memory::hybrid_search(
            store,
            crate::memory::HybridSearchOptions {
                query: query.to_string(),
                max_results,
                exclude_slugs: exclude.clone(),
                candidate_top_n: None,
                include_archived: false,
            },
        ) {
            return hits.into_iter().map(|h| (h.slug, h.summary)).collect();
        }
        return Vec::new();
    }
    crate::memory::grep_search_top_k(store, query, max_results, exclude, false)
        .ok()
        .map(|hits| hits.into_iter().map(|h| (h.slug, h.summary)).collect())
        .unwrap_or_default()
}

/// 全量重建用户记忆向量索引（供 `/memory reindex` 等调用）。
pub async fn reindex_user_memory(user_slug: &str) -> anyhow::Result<String> {
    let slug = user_slug.to_string();
    tokio::task::spawn_blocking(move || {
        let store = crate::memory::MemoryStore::new(&slug)
            .map_err(|e| anyhow::anyhow!("打开记忆库失败: {}", e))?;
        let embedder = crate::memory::MemoryEmbedder::new()
            .map_err(|e| anyhow::anyhow!("初始化 embedder 失败: {}（首次需联网下载模型）", e))?;
        let count = crate::memory::reindex_all(&store, &embedder)
            .map_err(|e| anyhow::anyhow!("重建索引失败: {}", e))?;
        Ok(format!("🧠 向量索引重建完成：{} 条", count))
    })
    .await
    .map_err(|e| anyhow::anyhow!("reindex 任务异常: {}", e))?
}

/// Async version that includes data catalog from backend.
pub async fn get_system_prompt_with_catalog(
    backend: &BackendClient,
    user_slug: &str,
    mem_opts: &MemoryInjectionOptions,
) -> String {
    let skills = crate::skills::load_skills();
    let skill_listing = crate::skills::build_skill_listing(&skills);

    // Keep the volatile data catalog at the tail to preserve prefix-cache hits
    // when only workspace data changes.
    let mut prompt = format!("{}{}", SYSTEM_PROMPT_BASE, skill_listing);
    append_memory_context(&mut prompt, user_slug, mem_opts);
    prompt.push_str(RICH_RENDERING_NOTE);
    prompt.push_str(VISUAL_GENERATION_POLICY_NOTE);
    prompt.push_str(DOCUMENT_HARNESS_NOTE);

    let catalog = build_data_catalog(backend).await;
    if !catalog.is_empty() {
        prompt.push_str("\n\n## 现有数据目录（title → id）\n");
        prompt.push_str(&catalog);
    }
    prompt
}

/// Enhance a frontend-supplied system prompt by appending skills listing and
/// correcting tool availability (the frontend prompt incorrectly claims bash is unavailable).
pub async fn enhance_system_prompt(
    prompt: &str,
    user_slug: &str,
    mem_opts: &MemoryInjectionOptions,
) -> String {
    let skills = crate::skills::load_skills();
    let skill_listing = crate::skills::build_skill_listing(&skills);

    let mut enhanced = prompt.to_string();

    // Remove the incorrect "no bash" claim from frontend prompt
    enhanced = enhanced.replace(
        r#"- 你**没有** sqlite/sqlite3、bash/shell、SQL 数据库、HTTP 请求、execute、run_command 等任何其他工具。**不要**假设、模拟或宣称要"运行"它们。"#,
        "- 你还有 **run_command** 工具可以执行 bash/shell 命令（如 ls、cat、grep、git 等）。",
    );

    if !skill_listing.is_empty() {
        enhanced.push_str(&skill_listing);
    }
    append_memory_context(&mut enhanced, user_slug, mem_opts);
    enhanced.push_str(RICH_RENDERING_NOTE);
    enhanced.push_str(VISUAL_GENERATION_POLICY_NOTE);
    enhanced.push_str(DOCUMENT_HARNESS_NOTE);
    enhanced
}

fn format_memory_stats(s: &crate::memory::MemoryStats) -> String {
    let vec_line = match s.vector_indexed {
        Some(n) => format!("{} 条", n),
        None => "无".to_string(),
    };
    format!(
        "schema_version: {}\n活跃条目: {}\n软删条目: {}\n归档条目: {}\n索引行: {}\nMEMORY.md: {} bytes\nPROFILE.md: {} bytes（{}）\n向量索引: {}",
        s.schema_version,
        s.active_entries,
        s.soft_deleted_entries,
        s.archived_entries,
        s.index_lines,
        s.index_bytes,
        s.profile_bytes,
        if s.has_profile { "有" } else { "无" },
        vec_line,
    )
}

// `/memory compact` 需要 LlmClient + UsageTracker，由 daemon session route 处理；这里只处理只读子命令。
async fn handle_memory_subcommand(parts: &[&str], config: &RuntimeConfig) -> CommandResult {
    let user_slug = config
        .user
        .as_ref()
        .and_then(|u| u.slug.clone())
        .unwrap_or_else(|| "local".to_string());
    let sub = parts.get(1).copied().unwrap_or("");
    match sub {
        "list" => match crate::memory::MemoryStore::new(&user_slug) {
            Ok(store) => match store.read_index_raw() {
                Ok(text) if text.trim().is_empty() => CommandResult {
                    output: "（暂无长期记忆）".to_string(),
                    continue_: true,
                },
                Ok(text) => CommandResult {
                    output: text,
                    continue_: true,
                },
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            },
            Err(e) => CommandResult {
                output: format!("Error: {}", e),
                continue_: true,
            },
        },
        "show" => {
            let slug = match parts.get(2).copied() {
                Some(s) if !s.is_empty() => s,
                _ => {
                    return CommandResult {
                        output: "Usage: /memory show <slug>".to_string(),
                        continue_: true,
                    };
                }
            };
            match crate::memory::MemoryStore::new(&user_slug) {
                Ok(store) => match store.read_entry(slug) {
                    Ok(entry) => CommandResult {
                        output: format!(
                            "# {}\n\n_{}_\n\n{}",
                            entry.slug, entry.summary, entry.content
                        ),
                        continue_: true,
                    },
                    Err(e) => CommandResult {
                        output: format!("未找到「{}」: {}", slug, e),
                        continue_: true,
                    },
                },
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            }
        }
        "user" => {
            let source = crate::identity::current_identity_source()
                .map(|s| s.label())
                .unwrap_or("unknown");
            CommandResult {
                output: format!("User slug: {} (source: {})", user_slug, source),
                continue_: true,
            }
        }
        "profile" => match crate::memory::MemoryStore::new(&user_slug) {
            Ok(store) => match store.read_profile() {
                Ok(text) if text.trim().is_empty() => CommandResult {
                    output: "（暂无 PROFILE.md）".to_string(),
                    continue_: true,
                },
                Ok(text) => CommandResult {
                    output: text,
                    continue_: true,
                },
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            },
            Err(e) => CommandResult {
                output: format!("Error: {}", e),
                continue_: true,
            },
        },
        "compact" => CommandResult {
            output: "/memory compact 仅在 REPL 内可用".to_string(),
            continue_: true,
        },
        "reindex" => match reindex_user_memory(&user_slug).await {
            Ok(msg) => CommandResult {
                output: msg,
                continue_: true,
            },
            Err(e) => CommandResult {
                output: format!("❌ {}", e),
                continue_: true,
            },
        },
        "stats" => match crate::memory::MemoryStore::new(&user_slug) {
            Ok(store) => match crate::memory::collect_stats(&store) {
                Ok(s) => CommandResult {
                    output: format_memory_stats(&s),
                    continue_: true,
                },
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            },
            Err(e) => CommandResult {
                output: format!("Error: {}", e),
                continue_: true,
            },
        },
        "migrate" => match crate::memory::MemoryStore::new(&user_slug) {
            Ok(store) => match crate::memory::migrate_store(&store) {
                Ok(report) => CommandResult {
                    output: format!(
                        "迁移完成：v{} → v{}，补 id {} 条，重写 {} 条",
                        report.from_version,
                        report.to_version,
                        report.ids_assigned,
                        report.entries_rewritten
                    ),
                    continue_: true,
                },
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            },
            Err(e) => CommandResult {
                output: format!("Error: {}", e),
                continue_: true,
            },
        },
        "import-markdown" => {
            let Some(root) = parts.get(2).map(std::path::PathBuf::from) else {
                return CommandResult {
                    output: "用法: /memory import-markdown <path>".to_string(),
                    continue_: true,
                };
            };
            match crate::memory::MemoryStore::new(&user_slug) {
                Ok(store) => match crate::memory::import_markdown_tree(&store, &root) {
                    Ok(report) => CommandResult {
                        output: format!(
                            "Markdown→Memory 导入完成：扫描 {} 个文件，新增 {} 段，更新 {} 段，未变化 {} 段，跳过 {} 个文件。源文件未修改。",
                            report.files_scanned,
                            report.chunks_imported,
                            report.chunks_updated,
                            report.chunks_unchanged,
                            report.files_skipped
                        ),
                        continue_: true,
                    },
                    Err(error) => CommandResult {
                        output: format!("导入失败: {}", error),
                        continue_: true,
                    },
                },
                Err(error) => CommandResult {
                    output: format!("打开记忆库失败: {}", error),
                    continue_: true,
                },
            }
        }
        _ => CommandResult {
            output: "Usage: /memory [list|show <slug>|profile|compact|reindex|migrate|import-markdown <path>|stats|user]"
                .to_string(),
            continue_: true,
        },
    }
}

pub async fn handle_command(
    input: &str,
    backend: &BackendClient,
    config: &RuntimeConfig,
) -> CommandResult {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return CommandResult {
            output: String::new(),
            continue_: true,
        };
    }

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let cmd = parts[0].to_lowercase();

    match cmd.as_str() {
        "/quit" | "/exit" | "quit" | "exit" => CommandResult {
            output: "Goodbye!".to_string(),
            continue_: false,
        },

        "/todo" => {
            let todo_type = parts.get(1).copied();
            match backend.get_data("todos").await {
                Ok(todos) => {
                    let arr = todos.as_array().unwrap_or(&Vec::new()).clone();
                    let filtered: Vec<_> = if let Some(t) = todo_type {
                        arr.into_iter()
                            .filter(|item| item["type"].as_str() == Some(t))
                            .collect()
                    } else {
                        arr
                    };
                    if filtered.is_empty() {
                        CommandResult {
                            output: "（暂无待办）".to_string(),
                            continue_: true,
                        }
                    } else {
                        let lines: Vec<String> = filtered
                            .iter()
                            .map(|t| {
                                let completed = t["completed"].as_bool().unwrap_or(false);
                                let title = t["title"].as_str().unwrap_or("Untitled");
                                let priority = t["priority"].as_str().unwrap_or("");
                                let due = t["dueDate"].as_str().unwrap_or("");
                                let mut line =
                                    format!("{} {}", if completed { "✅" } else { "⬜" }, title);
                                if !priority.is_empty() {
                                    line.push_str(&format!(" [{}]", priority));
                                }
                                if !due.is_empty() {
                                    line.push_str(&format!(" (截止: {})", due));
                                }
                                line
                            })
                            .collect();
                        CommandResult {
                            output: lines.join("\n"),
                            continue_: true,
                        }
                    }
                }
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            }
        }

        "/bookmark" | "/bookmarks" => match backend.get_data("categories").await {
            Ok(categories) => {
                let arr = categories.as_array().unwrap_or(&Vec::new()).clone();
                if arr.is_empty() {
                    CommandResult {
                        output: "（暂无书签）".to_string(),
                        continue_: true,
                    }
                } else {
                    let lines: Vec<String> = arr
                        .iter()
                        .map(|c| {
                            let emoji = c["emoji"].as_str().unwrap_or("");
                            let title = c["title"].as_str().unwrap_or("Untitled");
                            let empty_links = Vec::new();
                            let links = c["links"].as_array().unwrap_or(&empty_links);
                            let link_lines: Vec<String> = links
                                .iter()
                                .map(|l| {
                                    let lt = l["title"].as_str().unwrap_or("");
                                    let url = l["url"].as_str().unwrap_or("");
                                    format!("  - {}: {}", lt, url)
                                })
                                .collect();
                            format!("{} {}\n{}", emoji, title, link_lines.join("\n"))
                        })
                        .collect();
                    CommandResult {
                        output: lines.join("\n\n"),
                        continue_: true,
                    }
                }
            }
            Err(e) => CommandResult {
                output: format!("Error: {}", e),
                continue_: true,
            },
        },

        "/data" => {
            let key = parts.get(1);
            if key.is_none() {
                return CommandResult {
                    output: "Usage: /data <key>".to_string(),
                    continue_: true,
                };
            }
            match backend.get_data(key.unwrap()).await {
                Ok(value) => CommandResult {
                    output: serde_json::to_string_pretty(&value)
                        .unwrap_or_else(|_| "（无数据）".to_string()),
                    continue_: true,
                },
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            }
        }

        "/ls" => {
            let dir_path = parts.get(1).unwrap_or(&".");
            match backend.list_dir(dir_path).await {
                Ok(entries) => {
                    if entries.is_empty() {
                        CommandResult {
                            output: "（空目录）".to_string(),
                            continue_: true,
                        }
                    } else {
                        let lines: Vec<String> = entries
                            .iter()
                            .map(|e| {
                                let icon = if e.kind == "directory" {
                                    "📁"
                                } else {
                                    "📄"
                                };
                                let size_str = e
                                    .size
                                    .map(|s| format!(" ({} bytes)", s))
                                    .unwrap_or_default();
                                format!("{} {}{}", icon, e.name, size_str)
                            })
                            .collect();
                        CommandResult {
                            output: lines.join("\n"),
                            continue_: true,
                        }
                    }
                }
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            }
        }

        "/cat" => {
            let file_path = parts.get(1);
            if file_path.is_none() {
                return CommandResult {
                    output: "Usage: /cat <path>".to_string(),
                    continue_: true,
                };
            }
            match backend.read_file(file_path.unwrap()).await {
                Ok(result) => {
                    if result.is_binary {
                        let size = result.meta.map(|m| m.size).unwrap_or(0);
                        CommandResult {
                            output: format!("Binary file, size: {} bytes", size),
                            continue_: true,
                        }
                    } else {
                        CommandResult {
                            output: result.content.unwrap_or_else(|| "（空文件）".to_string()),
                            continue_: true,
                        }
                    }
                }
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            }
        }

        "/write" => {
            if parts.len() < 3 {
                return CommandResult {
                    output: "Usage: /write <path> <content>".to_string(),
                    continue_: true,
                };
            }
            let file_path = parts[1];
            let content = parts[2..].join(" ");
            match tokio::fs::write(file_path, content).await {
                Ok(()) => CommandResult {
                    output: format!("Written to {}", file_path),
                    continue_: true,
                },
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            }
        }

        "/rm" => {
            let file_path = parts.get(1);
            if file_path.is_none() {
                return CommandResult {
                    output: "Usage: /rm <path>".to_string(),
                    continue_: true,
                };
            }
            match tokio::fs::remove_file(file_path.unwrap()).await {
                Ok(()) => CommandResult {
                    output: format!("Removed {}", file_path.unwrap()),
                    continue_: true,
                },
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            }
        }

        "/search" => {
            let query = parts.get(1);
            if query.is_none() {
                return CommandResult {
                    output: "Usage: /search <query> [path]".to_string(),
                    continue_: true,
                };
            }
            let search_path = parts.get(2).copied();
            match backend.search_files(query.unwrap(), search_path).await {
                Ok(files) => {
                    if files.is_empty() {
                        CommandResult {
                            output: "未找到匹配的文件".to_string(),
                            continue_: true,
                        }
                    } else {
                        CommandResult {
                            output: files.join("\n"),
                            continue_: true,
                        }
                    }
                }
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            }
        }

        "/info" => {
            let info_path = parts.get(1);
            if info_path.is_none() {
                return CommandResult {
                    output: "Usage: /info <path>".to_string(),
                    continue_: true,
                };
            }
            match backend.get_file_info(info_path.unwrap()).await {
                Ok(info) => CommandResult {
                    output: format!(
                        "名称: {}\n类型: {}\n大小: {} bytes\n修改: {}\n创建: {}",
                        info.name, info.kind, info.size, info.modified, info.created
                    ),
                    continue_: true,
                },
                Err(e) => CommandResult {
                    output: format!("Error: {}", e),
                    continue_: true,
                },
            }
        }

        "/models" => {
            let models_str = if let Some(ref providers) = config.providers {
                providers
                    .iter()
                    .map(|p| format!("  - {}: {} ({})", p.name, p.model, p.protocol))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                "  (未配置)".to_string()
            };
            CommandResult {
                output: format!("可用模型:\n{}", models_str),
                continue_: true,
            }
        }

        "/config" => {
            let providers_str = config
                .providers
                .as_ref()
                .map(|ps| {
                    ps.iter()
                        .map(|p| format!("{} ({})", p.name, p.protocol))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "none".to_string());
            CommandResult {
                output: format!(
                    "Backend URL: {}\nProviders: {}\nActive: {}",
                    config.backend_url,
                    providers_str,
                    config.active_provider.as_deref().unwrap_or("none")
                ),
                continue_: true,
            }
        }

        "/memory" => handle_memory_subcommand(&parts, config).await,

        "/help" => CommandResult {
            output: r#"pwcli — Personal Workbench CLI

会话管理:
  /new              新建会话（清空当前对话）
  /session          显示当前会话信息
  /compact          压缩会话（减少 Token 用量）
  /branches         列出当前会话可切换的分支节点
  /branch <id>      切换分支并保留离开分支摘要
  /usage            显示 Token 用量统计
  /yolo             切换 YOLO 模式（跳过权限确认）

数据查询:
  /todo [type]      列出待办 (today/week/longterm)
  /bookmark         列出书签
  /data <key>       读取数据

文件操作:
  /ls [path]        列出目录
  /cat <path>       读取文件
  /write <path> <content>  写入文件
  /rm <path>        删除文件/目录
  /search <query> [path]   搜索文件
  /info <path>      文件元信息

长期记忆:
  /memory list      列出 MEMORY.md 索引
  /memory show <slug>  查看条目原文
  /memory profile   查看 PROFILE.md 用户画像
  /memory compact   强制压缩（跳过每日上限）
  /memory reindex   重建向量索引（index.db）
  /memory migrate   升级到 schema v3（类型/标签/来源）
  /memory import-markdown <path>  无损导入 Markdown 文件或目录（可重复执行）
  /memory stats     记忆库统计
  /memory user      查看当前 user_slug 与解析源

AI:
  /models           显示可用模型
  /config           显示配置
  /help             显示帮助
  /quit             退出

直接输入问题可与 AI 对话。"#
                .to_string(),
            continue_: true,
        },

        _ => CommandResult {
            output: String::new(),
            continue_: true,
        },
    }
}
