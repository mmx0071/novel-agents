# novelx-dev 参考（按需阅读）

主 Skill：[SKILL.md](SKILL.md)。本文展开反模式、模块落点与近期行为修正主题，避免把 SKILL 撑爆。

## 常见反模式

### 配置与门控

| 反模式 | 正确做法 |
|--------|----------|
| Rust/前端 `contains("继续写")` 等中文特判 | 改 `intents.yaml` / `gates.yaml` / `policies.yaml` |
| Agent 在正文伪造「1. 2. 3.」审批卡 | 只由服务端 `open_gate`；审校 FAIL 调 `offer_decisions` |
| 同轮连续 `apply=true` 绕过确认 | 预览 → 用户点卡 → 再 apply；审批卡确认的写章意图可带 `confirm_skip` |
| `sync_volume` / `confirm_setup` 再叠一层 mutation 确认 | 走专用门控，不叠第二层 |
| 硬规则阻断仍套「第N章/禁名」泛化话术 | 按 `content_rules` 真实 rule id / detail 生成 prompt |
| `gates.yaml` 选项 id 与 audit 裸 `"1"/"2"/"3"` 冲突 | 用稳定前缀 id（见 `gates.yaml` 注释） |
| 把 `audit_infra` 当正文问题局部修订 | 走 `audit_infra` 重试门控 |
| 审校已通过仍弹审校卡或称「未通过」 | P1/P2 是可改进项；用户要改直接 `revise_chapter` |
| 多章 `audit_chapters` 因软 `needs_user_choice`（形状等）停在一章，却说「复审通过/继续创作」 | 队列仅在一致性失败或硬规则/字数硬门停；通过后自动推进；队列未结束勿开 chapter_next「继续创作」 |
| 评审团 AutoRevise 后只调 `audit_chapter`，队列停在第 1 章 | 有活跃队列时必须 `audit_chapters continue`；通过则继续后续章，勿 `drain_council_auto_continue` 写下一章 |
| 多步任务只口头报进度、不推 To-dos | 递进任务用 `progressive_todo_list` + `emit_todos` / `PipelineEvent::TodoList`（审阅队列、批写等）；完成一项勾一项 |

### Studio 编排

| 反模式 | 正确做法 |
|--------|----------|
| 问进度却调 `continue_writing` | `list_plots`（± `get_project_status`） |
| 跳章/相位不对时静默改调别的工具 | 仍调目标工具拿硬拦 + 正确门控 |
| 一张剧情卡塞整卷 | `design_plot` 只切卷内一段；卷纲终止条件 ≥2 |
| `plot_acceptor` 改写收束条件来 pass | 对照卡面原文；`pass=true` 且发布成功才 `completed` |
| 单步 spawn 子 Agent 后推进 `next_chapter` | 发布收尾只在完整章流水线末尾一次 |
| 用 `spawn_agent(mode=continue\|revise\|audit_only)` 写章/审校 | 整章走 `continue_writing` / `revise_chapter` / `audit_*`（进程内流水线）；spawn 仅单步专精/只读旁路 |
| 润色 Agent 每章必跑 | `literary_editor` 靠 activation 建议或 Studio `activate_agents`；即时润色优先 `revise_chapter` |

### 格式与 Web

| 反模式 | 正确做法 |
|--------|----------|
| 为 Web 好看改落盘结构 / 另存第二份 | 落盘契约不变；展示走 `display_*` |
| 章纲手写 Markdown 当权威 | 只写 `outline.json` |
| 章列表有 outline 缓存就不拉正文 | `body_chars` 增长后须 refetch draft |
| preview 接口塞全文 draft | 列表只元数据；正文懒加载 |
| 卷末堆「卷末同步摘要」段落 | 直接写 `status`/`holdings` + `## 当前状态` |

### 超长篇性能

| 反模式 | 正确做法 |
|--------|----------|
| Entity/Bible 变更默认扫全部 draft | `longform.impact_scan_mode`；`impact_scan_all_on_setting` 默认 false |
| 把全部已写正文塞进单次 prompt | summaries + 局部 span + CanonContext 预算 |
| 一致性每章 full 上下文 | `audit_tier: layered`（接受偶发漏检 trade-off） |
| 单卷拖到 60–100 章无卷审 | 薄卷 20–40；中审软门 + handoff 卷审 |

## 改哪里（速查）

| 要改的行为 | 首选落点 |
|------------|----------|
| Studio 开关 | `config/features.yaml` + `novelx-core` features 读取 |
| 固定审批卡文案/按钮 | `config/gates.yaml` |
| 写章/批写/进度等短语路由 | `config/intents.yaml` |
| 正文硬规则词表/阈值 | `config/content_rules.yaml` + `novelx-harness::content_rules` |
| 章长/硬门 | `config/chapter.yaml` |
| 批写上限/审计档/impact 模式 | `config/longform.yaml` |
| 卷中审阈值 | `config/volume.yaml` |
| 章步骤顺序/专改 | `config/pipeline.yaml` |
| Agent 注册/激活建议 | `config/agents.yaml` + `activation-policy.md` |
| Root 编排话术 | `config/skills/studio.md` |
| 落盘格式契约 | `schemas/*` + `content-formats.md` |
| mutation 预览/确认 | `novelx-tools` mutation + features |
| studio_next 情境卡 | `novelx-core::studio_next` + `studio_next_gates` |
| 批写循环 | `novelx-pipeline::batch` |
| 无人值守软相位 | `config/unattended.yaml` + `studio.unattended_soft_skip`；`novelx-harness::UnattendedPolicy` |
| 卷 QA / 伏笔相位 | `volume_qa_phase` / `foreshadow_phase`（`novelx-pipeline::volume_qa` / `foreshadow_phase`） |
| body_state 板 | `novelx-pipeline::body_state` |
| 阅读 API / display | `novelx-app-server` + `schemas::display_*` |
| Web 章缓存/懒加载 | `web/src/App.jsx` |

## 共享 Skill 注入

运行时按 Agent 白名单前缀注入共享短文，**勿**在各 `agents/*/SKILL.md` 复制：

- `prose-pitfalls` — 正文硬雷区
- `content-formats` — 落盘格式
- `volume-lifecycle` — 卷相位 / 衔接章

注入与预算逻辑：`novelx-skills`。

## 近期行为修正主题（维护对照）

| 主题 | 要点 |
|------|------|
| gates 外置 + studio_next | 固定门控 YAML 化；情境下一步白名单工具卡；减少「僵硬」固定三按钮 |
| 预期事件 | `expectation_reviewer` + 工具链；硬条件满足时检阅，不进章流水线 order |
| longform / batch | 批写到卡点、审计档、冷归档、volume_* 模块、activation_hints |
| body_state / world_state | 伤势·能力载体确定性板；硬规则文案保真 |
| 内容 schema 双层 | prompt 软约束 + Rust schema 硬校验；migrate 命令 |
| Web 正文空 | 懒加载与 `body_chars` 缓存失效修复 |
| 作品中立 | `projects/` 不进仓；框架禁专名 |

## 文档交叉引用

- 架构总览：仓库根 `README.md`
- 配置边界：`config/README.md`
- Studio：`config/skills/studio.md`
- 格式：`config/skills/content-formats.md`、`config/schemas/README.md`
- 卷生命周期：`config/skills/volume-lifecycle.md`
- 激活策略：`config/skills/activation-policy.md`
- 创作工作流：`.cursor/skills/create-novel/SKILL.md`
