# 阅读区内容格式契约（题材中立）

机器校验以 Rust `novelx_pipeline::schemas` 为准；本文供人阅读。

## 校验策略

| 入口 | 策略 |
|------|------|
| Web `PUT /content` / 手工编辑 | **硬拒**：不合 schema 不写入 |
| 流水线 `writer`（`continue_writing` / revise） | **先保留再修正**：立刻落盘 → 确定性归一 → 必要时 LLM 形状修正 → 仍不合规则保留草稿并拦发布 |
| 发布门控 | 仍要求正文通过严格 `validate_draft`（含 ≥800 字等） |

## 各页签格式

| 页签 | 落盘 | 格式要点 |
|------|------|----------|
| 正文 | `chapters/NNN/draft.md` | 首行 `# 第N章 …`（阿拉伯数字；流水线会把「第一章」等归一）；正文≥800字；禁止整篇 ` ```json ` |
| 章纲 | `chapters/NNN/outline.json` | JSON 必填：`title, pov, time_location, goal, conflict, emotion_curve, key_events[], characters[], scene_tags[], cliffhanger, lore_queries[]`；`key_events`≥2 |
| 总纲 | `artifacts/master_outline.md` | 必含 H2：`一句话卖点`（或 Logline）、`三幕结构`/`分卷`、`主角弧`、`主线冲突` |
| 剧情卡 | `plots/*.md` | FM：`title, scope, plot_type, status, needs_bridge`；正文 H2：`概览`、`剧情走向`、`冲突与赌注`、`出场人物`、`收束条件` |
| 人物/物品/地点 | `entities/{group}/*.md` | FM：`name, status`；人物：`History/Personality/Core events/Current status`；物品：`Origin/Usage/Current status`；地点：`Overview/Factions/Production`（中英标题别名可归一） |
| 设定缺口 | （无文件） | API `string[]` → 固定前缀 + `- ` 列表 |
| 卷纲 | `artifacts/arc_outline.md` | H1 `# 第X卷 · …`；必含卷定位/开卷状态/冲突升级阶梯/关键节点/人物弧/伏笔/卷末终止条件(≥2条)/卷末交付 |
| 世界观 | `artifacts/bible.md` | H1 `# 世界观`；至少 `## 0.` `## 1.` `## 2.` `## 7.` |

`story_outline.json` 仅存结构化 acts，不单独作为阅读区「总纲」页签。

旧项目若仍有 `outline.md`（fenced JSON），读取时会尝试解析并一次性写出 `outline.json`。
