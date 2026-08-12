# NovelX

用对话写长篇：立项、写章、审阅、改稿，一条链路走完。面向想稳定连载的作者，也可支撑数百章规模的卷式推进。

## 能做什么

- **立项**：说清灵感 → 总纲 / 卷纲 / 设定 → 确认后开始写章
- **写章**：按卷推进；可一章一章写，也可批量续写到需要你拍板的地方
- **审阅**：逐章或整卷检查前后文、节奏与设定是否对得上
- **改稿**：按你的要求局部改或整章修订；重要设定改动会先给你确认
- **写作台**：左侧看大纲 / 设定 / 正文，右侧用创作助手对话推进

题材与作品内容由你决定；框架本身不绑定某一本书。

## 快速开始（Web）

1. 准备 API Key：复制 `.env.example` → `.env`，填入 `DEEPSEEK_API_KEY`（也可稍后在 Web「配置 → 模型」粘贴）
2. 启动：

```bash
# 终端 1：后端（默认 127.0.0.1:8765）
cargo run -p novelx-cli -- web

# 终端 2：前端
cd web && npm install && npm run dev
# → http://127.0.0.1:5173
```

3. 在页面里新建作品，用自然语言告诉助手你想写什么。

一键重启（编译 + 启动）：

```bash
./scripts/restart.sh              # macOS / Linux（默认后台 daemon，日志 .novelx/web.log）
./scripts/restart.sh --quick      # 不重新编译
./scripts/restart.sh --foreground # 前台运行
```

```powershell
.\scripts\restart.ps1
.\scripts\restart.ps1 -Quick
```

生产可先 `cd web && npm run build`，再只启 `novel web`（静态资源由服务端托管）。

---

## 开发者指南

以下面向改框架、跑 CLI、接配置的人。作者日常使用请看上文。

### 特性（工程视角）

- Codex 式运行时：CLI / Web → Session → tools；协作角色按需启动
- Skills 渐进披露：启动只注入摘要，激活后再加载全文
- 章流水线可配置：`config/pipeline.yaml`；扩展角色按需激活
- 局部改稿优先；立项与卷生命周期门控；批写到卡点；Studio 安全闸
- **Loop 外环（Goal）**：`continue_writing_batch` / `novel run --batch` 按 Trigger→Frame→Run→Verify→Record→Stop 运转；停机认 `StopContract`（硬门永不自动跳过）；校验仍以进程内 pipeline + harness 为准

### 依赖

- Rust 1.75+（推荐最新 stable）
- Node.js 18+（前端开发）
- DeepSeek API Key（可选；无 Key 时占位模式）

### CLI

```bash
cargo build -p novelx-cli --release
cargo run -p novelx-cli -- --help

cargo run -p novelx-cli -- init my-novel --genre 未定
cargo run -p novelx-cli -- run my-novel 1
cargo run -p novelx-cli -- run my-novel 1 --revise --instructions "改第2段，加强冲突"
cargo run -p novelx-cli -- run my-novel --batch --max-chapters 10
# 批写到卡点：遇硬门即停；软相位默认见 config/unattended.yaml

cargo run -p novelx-cli -- status my-novel
cargo run -p novelx-cli -- projects
cargo run -p novelx-cli -- agents
cargo run -p novelx-cli -- migrate my-novel
```

Release 二进制：`./target/release/novel`。

### Loop 外环与无人值守批写

连写不是「模型说写完了就停」，而是机器可读的 Goal 外环：

| 阶段 | NovelX 落点 |
|------|-------------|
| Trigger | 意图 / 审批卡「连写到卡点」/ CLI `--batch` / Web 在线唤醒 |
| Frame | `projects/<name>/.novelx/loop/job.json`（预算、`until_chapter`、软跳过键） |
| Run | **按章**执行：每轮只对 `next_chapter` 跑一次 `execute_pipeline` |
| Verify | 一致性 P0 / content_rules / 字数硬门等（与单章发布链相同） |
| Record | `state.json` + memory + `loop/journal.jsonl` |
| Stop | `StopContract`（兼保留 `stopped_reason` 字符串给旧客户端） |

**粒度与自然停**：执行最小单位是**章**；无人值守的自然停单位 ≈ **当前剧情卡**——本卡收束后不会自动建/升下一张卡；下一卡未设计、未激活或缺少可用收束条件时，以 `plot_gate:*`（如 `need_design_plot` / `planned_inactive` / `missing_exit`）硬停，需人 `design_plot` + `update_plot(in_progress)` 后再批写。`batch_max_chapters` 只是安全上限，不是鼓励跨多卡冲配额。「卡点」= 硬门/剧情卡门控停机，不是「写满 N 章」。

质量红线：**硬门**（setup / 卷交接 / 章序 / 剧情门 / 一致性 P0 / 字数硬门 / 草稿形状 / handoff 审）永不自动跳过。软相位跳过只走 `config/unattended.yaml`，并写入 journal 可审计。

Web 服务存活时会周期性扫描可恢复任务（`config/longform.yaml` → `loop_wake_interval_secs`，`0` 关闭）：仅崩溃续跑或作者已「允许自动续写」（`POST /api/projects/{name}/loop/arm`）才会再跑；项目有活跃 Turn 则跳过。创作状态栏可看「连写：可续写 / 需处理」。

批结束若 Goal/Quota 且本批发布数 ≥ `loop_end_verify_min_chapters`，会做确定性卷 QA 相位抽检（短剧跳过）；需人处理则阻断自动唤醒。

### LLM 配置

1. `.env` 中的 `DEEPSEEK_API_KEY`
2. 任务 / 模型映射：`config/llm.yaml`（`NOVELX_LLM_PROFILE=dev|prod` 可覆盖）
3. Agent Skills：`config/skills/**/SKILL.md`
4. Web「配置 → 模型」可粘贴 Key（只写不读）

**Loop / LLM 重试与降级（摘要）**

| 机制 | 配置 | 行为 |
|------|------|------|
| 瞬态重试 | `llm.yaml` → `max_retries` 等 | 429/5xx/网络错误退避重试 |
| 同端点换模 | 任务级 `fallback_model` | 主模型重试耗尽后换模再跑一套预算 |
| 兼容端点链 | `failover.enabled` + `failover.chain` | 再按序试另一 OpenAI 兼容 `base_url`（有 Key 才试；占位无 Key 不进） |
| 批写审计 infra | `longform.yaml` → `batch_max_audit_infra_retries` | 空响应/不可解析自动 `AuditOnly` 再审；耗尽 → `StopContract(audit_infra)`（勿当正文局部修订） |
| Studio 终败 | `gates.yaml` → `llm_turn_retry` | 内层重试+failover 后 Studio 再自动开 1 轮；仍失败出「再试一次 / 结束本轮」 |

### 章流水线（摘要）

默认顺序见 `config/pipeline.yaml`：

```
chapter_planner → lore_librarian → writer
→ nomenclature_curator → dialogue_specialist → scene_specialist
→ pacing_reviewer → literary_editor → consistency_auditor
→ foreshadow_tracker → summarizer → plot_acceptor
```

典型立项：`lock_brief` → 总纲 / 卷纲 → Bible → `confirm_setup` → 写章；卷末 `sync_volume`。

### 架构

```
crates/
  novelx-protocol/     # Submission / Op / Event / AgentPath
  novelx-skills/       # 渐进披露 loader
  novelx-draft-patch/  # 局部段落补丁
  novelx-harness/      # gates / 硬规则 / 激活 / longform / unattended
  novelx-llm/          # OpenAI-compatible 客户端
  novelx-pipeline/     # 领域单步 + schemas / batch / loop_runtime
  novelx-tools/        # continue / revise / spawn_agent …
  novelx-core/         # Codex Session：submission_loop + SubAgent + loop wake
  novelx-app-server/   # axum HTTP + WS → Submission；在线 loop 扫描
  novelx-cli/          # novel 二进制
config/                # agents / pipeline / features / longform / unattended / skills …
projects/<name>/       # 小说数据（state、chapters、entities、artifacts、.novelx/loop…）
web/                   # NovelX React 前端
```

### 决策/执行审计（ops journal）

Studio 在 `projects/<name>/.novelx/ops_journal.jsonl` 追加记录工具调用、门控选择、mutation 预览/确认/应用等。`ChatHistoryReset` 只清聊天，不删 journal。开关：`config/features.yaml` → `studio.ops_journal`。

- CLI：`novel ops-log <project> [--limit N] [--kind mutation_applied] [--chapter 3] [--json]`
- HTTP：`GET /api/projects/{name}/ops_journal?limit=&chapter=&kind=&after=`

批写外环另有 `projects/<name>/.novelx/loop/job.json` + `journal.jsonl`（StopContract、软跳过键、批结束抽检）。HTTP：`GET /api/projects/{name}/loop`，武装自动续写：`POST /api/projects/{name}/loop/arm`。

| 文件 | 作用 |
|------|------|
| `config/agents.yaml` | Agent 注册与激活条件 |
| `config/pipeline.yaml` | 章步骤顺序与 handler |
| `config/features.yaml` | Studio / 流水线特性开关 |
| `config/longform.yaml` | 质量档、审计档、批写上限、影响扫描、`loop_*` 唤醒/抽检 |
| `config/unattended.yaml` | 无人值守软相位跳过策略 |
| `config/chapter.yaml` | 章长目标与字数硬门 |
| `config/volume.yaml` | 卷中审 / 厚卷频率 |
| `config/gates.yaml` / `intents.yaml` | 门控与确定性意图路由 |

更细见 [`config/README.md`](config/README.md)、[`config/schemas/README.md`](config/schemas/README.md)。

### Cursor Skills

| Skill | 用途 |
|-------|------|
| [`.cursor/skills/create-novel/SKILL.md`](.cursor/skills/create-novel/SKILL.md) | 用本系统写长篇（工作流） |
| [`.cursor/skills/novelx-dev/SKILL.md`](.cursor/skills/novelx-dev/SKILL.md) | 改框架：题材中立、配置/门控分层、Studio 闸 |
| [`.cursor/skills/novelx-dev/reference.md`](.cursor/skills/novelx-dev/reference.md) | 开发反模式与「改哪里」速查 |
