# 阅读区内容格式契约（题材中立）

机器校验以 `web/src/deskCheck.js`（`novelx_check`）为准。  
生成锚定：预设 [`content-formats`](../../integrations/dsh-preset-novelx/preset/skills/content-formats/SKILL.md)。

## 双层原则

| 层 | 目标 |
|----|------|
| 落盘 | 固定路径 + 字段/H2，便于 Agent 加载与过滤（status/holdings 等） |
| Web | 阅读台展示（去 FM、章纲 JSON→MD、设定卡 H2 中文）；不改落盘结构 |

## 各页签格式

| 页签 | 落盘 | 格式要点 |
|------|------|----------|
| 正文 | `chapters/NNN/draft.md` | 首行 `# 第N章 …`；发布字数见 `chapter.yaml`（默认硬门 ≥4500） |
| 剧本（短剧） | `episodes/NNN/script.md` | 首行 `# 第N集 …`；≥2 个 `## 场`；每场≥1 条`【画面】`；须有`【钩子】`；见 `content-formats-script` |
| 章纲 | `chapters/NNN/outline.json` | JSON 必填见 content-formats；推荐 `plot_includes[]`/`plot_defers[]`/`items[]`/`locations[]` |
| 总纲 | `artifacts/master_outline.md` | H2：`一句话卖点`、`三幕结构`/`分卷`、`主角弧`、`主线冲突` |
| 剧情卡 | `plots/*.md` | FM：`title, scope=local, plot_type, status, needs_bridge`；H2：概览/剧情走向/冲突与赌注/出场人物/收束条件 |
| 人物/物品/地点 | `entities/{group}/*.md` | FM：`name, status`（+`holdings`）；落盘 H2 英文 canon |
| 卷纲 | `artifacts/arc_outlines/{NN}.md` | H1 `# 第X卷 · …`；必含卷定位与卷末终止条件(≥2) |
| 世界观 | `artifacts/bible.md` | H1 `# 世界观…`；至少 `## 0./1./2./7.` |

`story_outline.json` 仅存结构化 acts，不单独作为阅读区「总纲」。

卷末同步：直接写实体 `status`/`holdings` 与「当前状态」，不堆「卷末同步摘要」。

旧项目若仍有 `outline.md`（fenced JSON），用 `novelx_migrate` 迁到 `outline.json`。
