# 阅读区内容格式契约（题材中立）

机器校验以 Rust `novelx_pipeline::schemas` 为准。  
**生成锚定 Skill**：[`config/skills/content-formats.md`](../skills/content-formats.md)（写章/规划/设定类 Agent 自动注入）。

框架级 YAML / Skills 的「可编辑配置 vs 引擎」说明见 [`config/README.md`](../README.md)。

## 双层原则

| 层 | 目标 |
|----|------|
| 落盘 | 固定路径 + 字段/H2，便于 Agent 加载与过滤（status/holdings 等） |
| Web | `display_*` 简洁易读（去 FM、章纲 JSON→MD、设定卡 H2 中文）；不改落盘结构 |

## 校验策略

| 入口 | 策略 |
|------|------|
| Web `PUT /content` / 手工编辑 | **硬拒**：不合 schema 不写入 |
| 流水线 `writer` / `script_writer` | **先保留再修正**：落盘 → 归一 → 必要时修形 → 仍不合则拦发布 |
| 发布门控（长篇） | 正文须通过 `validate_draft`（结构）+ `chapter.yaml` 字数硬门（默认 ≥4500、≤11000）+ 一致性/硬规则；超长可 `split_chapter` |
| 发布门控（短剧） | 剧本须通过 `validate_script`（标题/`## 场`/`【画面】`/`【钩子】`）+ `script.yaml` 字数与形状旋钮；散文无画面不可发布 |

## 各页签格式

| 页签 | 落盘 | 格式要点 |
|------|------|----------|
| 正文 | `chapters/NNN/draft.md` | 首行 `# 第N章 …`；结构校验见 `validate_draft`；发布字数见 `chapter.yaml`（默认硬门 ≥4500）；禁止整篇 \`\`\`json |
| 剧本（短剧） | `episodes/NNN/script.md` | 首行 `# 第N集 …`；≥2 个 `## 场`；每场≥1 条`【画面】`（总数门槛见 `script.yaml`）；须有`【钩子】`；标题禁 \`\`\`/`markdown`；见 `validate_script` / `content-formats-script` |
| 章纲 | `chapters/NNN/outline.json` | JSON 必填见 content-formats；推荐 `plot_includes[]`/`plot_defers[]`/`items[]`/`locations[]`；新生成 `key_events`≤4，旧稿可读可 Web 保存；Web 由 `display_chapter_outline` 渲染；Agent 用 `revise_outline` 修订（预览确认） |
| 总纲 | `artifacts/master_outline.md` | H2：`一句话卖点`、`三幕结构`/`分卷`、`主角弧`、`主线冲突` |
| 剧情卡 | `plots/*.md` | FM：`title, scope=local, plot_type, status, needs_bridge`；H2：概览/剧情走向/冲突与赌注/出场人物/收束条件 |
| 人物/物品/地点 | `entities/{group}/*.md` | FM：`name, status`（+`holdings`）；落盘 H2 英文 canon；Web 展示中文（经历/性格/…） |
| 设定缺口 | （无文件） | API `string[]` → 固定前缀 + `- ` 列表 |
| 卷纲 | `artifacts/arc_outlines/{NN}.md` | H1 `# 第X卷 · …`；必含卷定位…卷末终止条件(≥2)/卷末交付 |
| 世界观 | `artifacts/bible.md` | H1 `# 世界观…`；至少 `## 0./1./2./7.` |

`story_outline.json` 仅存结构化 acts，不单独作为阅读区「总纲」。

卷末同步：直接写实体 `status`/`holdings` 与「当前状态」，不堆「卷末同步摘要」。

Studio 主动修正（正文/纲/实体/世界观）走「写前审计 → 确认落盘 → 依赖面 impact 扫描 → 可选级联修订」；见 `config/skills/studio.md` 与 `studio.impact_cascade`。

旧项目若仍有 `outline.md`（fenced JSON），读取时会尝试解析并写出 `outline.json`。
