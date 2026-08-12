---
name: novelx-dev
description: >-
  NovelX / novel-agents 框架开发规范：题材中立、Prompt/Schema/门控分层、
  features 与 YAML 优先、Studio 安全闸、超长篇吞吐、落盘/Web 双层、
  crate 边界与常见反模式。Use when editing Rust crates, Web UI, config/skills,
  harness rules, schemas, tests, pipeline, Studio gates/intents, or any product
  code in this repo. Prefer loading before changing prompts, defaults, fixtures,
  reader formats, or agent loop behavior.
---

# NovelX 项目开发规范

约束 **产品代码与框架配置**。创作工作流见 [create-novel](../create-novel/SKILL.md)。  
细节与反模式扩写见 [reference.md](reference.md)。

## 改代码前决策树

1. **行为开关 / 文案 / 意图短语** → 改 `config/features.yaml`、`gates.yaml`、`intents.yaml`、`policies.yaml`、`content_rules.yaml`，勿在 Rust/前端加中文特判。
2. **结构校验 / 阅读区格式** → 改 `novelx_pipeline::schemas` + 同步 `config/skills/content-formats.md` + `config/schemas/README.md`（必要时 `migrate`）。
3. **Agent 话术 / 编排** → 改 `config/skills/**`；共享短文靠白名单注入，勿在各 Agent SKILL 复制长文。
4. **章步骤** → 优先 `config/pipeline.yaml`（`order` / `mvp` / `handlers`）；`specialist_rewrite` 不必改 Rust。
5. **仍须改引擎** → 按 crate 边界落点（见下），并补回归测试。

## 题材 / 作品中立（硬规则）

1. 不得在 `crates/**`、`web/**`、`config/**` 写死某一作品的书名、角色、势力、地名、梗概或卷名。
2. 样例只在 `projects/<name>/`；框架经路径 / `project` 参数读写，不假设当前书。
3. 测试用中性占位：`sample-novel`、`demo`、`主角`、`甲`；情节用抽象句，勿抄现成小说。
4. Prompt / Skill 不预设玄幻/修仙等默认世界观；由 brief + Bible 决定。
5. 集成/迁移测试勿绑死 `projects/某书名`；优先临时目录最小骨架；可选扫 `projects/` 须枚举且失败 skip。
6. `body_state` / 硬规则只抓结构 cue（侧别、载体等），不写故事专名。

违规：改为通用占位或下沉 `projects/`，并补回归证明框架不依赖该书名。

## Prompt vs Schema vs 确定性门控

| 层 | 权威 | 用途 |
|----|------|------|
| Prompt / Skill | `config/skills/**` | 软约束：写章目标、编排、Agent 行为 |
| Schema | `novelx_pipeline::schemas` | 结构硬校验；人读摘要在 `content-formats.md` / `schemas/README.md` |
| 确定性门控 | harness + `content_rules` / `chapter.yaml` / phases | 发布拦、跳章、setup/volume 相位、字数、一致性 P0 |
| Loop 外环 | `loop_runtime` + `longform.loop_*` | Goal 批写 StopContract；硬门不可自动化；在线 wake / journal |
| 意图路由 | `intents.yaml` + `novelx-core::intent` | 高置信写章/批写/修订等薄匹配；未命中才进 LLM tool loop |
| 人机选项 | `gates.yaml` + `offer_decisions` | 固定事务按钮 vs 情境 `studio_next` vs 审校 `audit` |

- Web `PUT /content`：**硬拒**不合 schema。
- 流水线 writer：**先保留再修正**（normalize → 仍不合则拦发布）。
- `content_rules`：`enabled=false` 跳过；`blocking=false` 仅警告。词表/阈值/文案在 YAML，Rust 只做通用扫描。

## 配置与 features（优先于硬编码）

- **可热重载**：`content_rules`、`naming_rules`、`policies`、`llm.yaml`、框架 `skills/**`（见 `config/README.md`）。
- **布尔行为** → `config/features.yaml`（setup/volume/章序/mutation/impact/批写归档等）。
- **枚举与阈值** → `longform.yaml` / `chapter.yaml` / `volume.yaml` / `continuity.yaml`。
- API Key **只进** `.env`；GET 永不回传明文。
- 改 Studio/批写/扫描范围：先改 YAML，再考虑引擎。

## Crate 边界

| Crate | 职责 |
|-------|------|
| `novelx-protocol` | Submission / Op / Event / AgentPath |
| `novelx-core` | Session、intent/gates/features、studio_next、线程与 UI 门控 |
| `novelx-harness` | content_rules 引擎、activation、longform/字数、continuity |
| `novelx-pipeline` | `run_step`、schemas、batch、body_state、cards、volume_*、memory/impact |
| `novelx-tools` | 工具分发、mutation 预览/确认、continue/revise/audit |
| `novelx-skills` | 渐进披露 loader + 注入预算 |
| `novelx-draft-patch` | 段级局部修订 |
| `novelx-app-server` / `novelx-cli` | HTTP+WS / CLI 入口 |

编排权在 **core**；领域单步在 **pipeline**。无独立 Orchestrator LLM 输出 pipeline JSON。

## Studio 安全闸（实现时勿绕过）

对应 `features.yaml` 开关（默认多为 true）：

- `enforce_setup_gate`：`setup_phase != ready` 硬拦写新章
- `enforce_volume_phase`：非 `drafting_volume` 拦 `continue_writing` / `design_plot`
- `enforce_chapter_order`：跳章 / 前章缺口硬拦（修订已有章不受卷相位限制）
- `require_mutation_confirm`：改盘工具先预览；用户确认后 `apply=true`+`mutation_id`；**禁止同轮连续 apply 绕过**
- `impact_cascade`：应用后扫依赖 → `confirm_impact`；按 设定→纲→章纲→正文 级联
- 审批卡只由服务端 `open_gate` 渲染；Agent **禁止**在正文伪造编号卡
- 问进度 → `list_plots`（± `get_project_status`），**禁止**误调 `continue_writing`
- 跳章/相位不对时仍须调目标工具拿硬拦+正确门控，勿静默改调别的工具

## 超长篇约束（800–1000 章 × 5–6k）

- 批写：`continue_writing_batch` / `--batch`；上限 `longform.batch_max_chapters`；遇硬门即停；软门默认见 `unattended.yaml`。
- `audit_tier: layered`：常规章轻量一致性上下文；高潮/奇数章/复审 full。
- `quality_tier` + `pipeline.longform_lean`：专改/伏笔过滤。
- 字数：目标 5000–6000；硬门默认 ≥4500；连续 SoftShort 可升格阻断。
- 卷中审软门 / 卷末 sync 前卷审：见 `volume.yaml` + features。
- **impact 默认勿全书扫 draft**（用 `longform.impact_scan_mode: indexed|volume|all`）。
- 冷归档：`studio.cold_archive_drafts`；已完成卷 draft 可 gzip 离热路径。
- Web 章列表：preview 只带 `has_draft`/`body_chars`；正文 `GET /chapters/{n}` 懒加载；`body_chars` 增长须 refetch。

## 内容双层（落盘 vs Web）

- 生成只写**落盘**形态；Web 用 `display_*` 渲染，**勿为好看改落盘或另存第二份**。
- 章纲只写 `outline.json`；展示走 `display_chapter_outline`。
- 实体：落盘 FM + 英文 H2 canon；Web 去 FM、H2 中文；sync 写 `status`/`holdings` + `## 当前状态`，不堆「卷末同步摘要」。
- `story_outline.json` 仅存 acts，不是阅读区总纲。
- 机器以 schemas 为准；改格式须同步 Skill + migrate。

## 流水线与发布

- 顺序以 `pipeline.yaml` 为准：先生产 → 专改 → 一致性 → 摘要 → `plot_acceptor`。
- 一致性 P0 **挡发布**；节奏 P0 **不挡**；`plot_acceptor` 未收束 **不挡**发布（跨多章铺垫）；通过并发布成功才 `completed`。收束后**不**自动升 `next_plot`（Loop 自然停在剧情卡边界；跨卡须人 prep）。
- 发布收尾只在**完整章流水线末尾**一次；单步 spawn 不推进 `next_chapter`。
- 发布链：`validate_draft` → 字数硬门 → `content_rules` → 一致性 P0；缺收束条件硬拦，未收束仅记录。
- 门控文案须吃透真实 `rule id` / `gate_prompt`，勿用泛化「第N章/禁名」fallback 盖掉细节。

## 测试与迁移

- `novel migrate <project>`：阅读区格式迁移（outline.md→json 等），**不发明情节**。
- schema 单测在 `novelx-pipeline`；改 schema 同步 `content-formats.md` + migrate。
- 发现违规中立或绕过门控：改通用实现并补回归。

## 常见反模式（近期修正沉淀）

完整列表见 [reference.md](reference.md#常见反模式)。开发时至少避开：

1. 在 Rust/前端硬编码意图短语或门控按钮文案  
2. 同轮 `apply=true` 绕过 mutation 确认  
3. 把 `audit_infra`（空响应/解析失败）当正文问题去做局部修订  
4. 审校已通过（仅有 P1/P2）仍弹审校门控或称「未通过」  
5. 为 Web 手写第二份章纲 Markdown / 改落盘结构  
6. impact / Entity·Bible 变更默认扫全部 draft  
7. `gates.yaml` 选项 id 与 audit 裸 `"1"/"2"/"3"` 冲突  
8. 把一张剧情卡当整卷；或 `plot_acceptor` 改写收束条件来 pass  

## 与其它规范的关系

| 主题 | 权威 |
|------|------|
| Studio 编排 | `config/skills/studio.md` |
| 落盘/展示 | `config/skills/content-formats.md`、`config/schemas/README.md` |
| 卷/剧情节奏 | `config/skills/volume-lifecycle.md` |
| 流水线/激活 | `config/skills/activation-policy.md`、`config/pipeline.yaml` |
| 配置编辑边界 | `config/README.md` |
| 架构总览 | `README.md` |
| 创作工作流 | `.cursor/skills/create-novel/SKILL.md` |

## 项目模式（longform / short_drama）

- `meta.json` → `project_mode`：`longform`（默认）或 `short_drama`（AI 漫剧剧本）。
- 短剧落盘 `episodes/NNN/script.md` + `pipeline-script.yaml`；**勿**套用 `chapter.yaml` 4500 字门与 volume/batch 工具。
- 行为差异优先 YAML（`script.yaml` / `pipeline-script.yaml` / intents），保持题材中立。
