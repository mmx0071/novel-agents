# novelx-dev 参考（按需阅读）

主 Skill：[SKILL.md](SKILL.md)。本文展开反模式与「改哪里」速查。

## 常见反模式

### 检查与格式

| 反模式 | 正确做法 |
|--------|----------|
| 前端 / 检查工具写死某书专名 | 抽成通用规则；词表进 `naming_rules.yaml` / `content_rules.yaml` |
| 硬规则阻断却用泛化话术 | 按 `content_rules` 真实 rule id / detail 回报 |
| 为 Web 好看改落盘结构 / 另存第二份 | 落盘契约不变；展示走 `deskDisplay.js` |
| 章纲手写 Markdown 当权威 | 只写 `outline.json` |
| 章列表有 outline 缓存就不拉正文 | `body_chars` 增长后须 refetch draft |
| preview 接口塞全文 draft | 列表只元数据；正文懒加载 |
| 卷末堆「卷末同步摘要」段落 | 直接写 `status`/`holdings` + `## 当前状态` |

### 编排

| 反模式 | 正确做法 |
|--------|----------|
| 有 blocker 仍推进 `next_chapter` | 先 `novelx_check`；blocker 告诉用户 |
| 一张剧情卡塞整卷 | 只切卷内一段；卷纲终止条件 ≥2 |
| `plot_acceptor` 改写收束条件来 pass | 对照卡面原文；`pass=true` 才 `completed` |
| 章纲 `plot_defers` 已写「不兑现收束」仍 completed | 不得 pass |
| 主 agent 自己写正文 | 只路由：`novelx_progress` / `novelx_canon` / `novelx_write` |

### 框架 vs 作品

| 反模式 | 正确做法 |
|--------|----------|
| 手改 `projects/<书>/` 来修漏同步 / 伤势 / 地点折叠 | 改 dsh 预设 Skill 或 `deskCheck` / `deskMigrate`（中性）→ 新开 NovelX 会话 |
| 开发 Agent 直接写剧情、补人物卡、改收束条件 | 创作走 [create-novel](../create-novel/SKILL.md) |
| 单测从现成书抄书名、角色、章题、剧情卡标题 | `sample-novel` / `主角` / `甲` / `信物` / `样例小区` |
| 把某书的同步事故写成特判 | 抽成通用规则（必更名单、主名优先于 alias、roster 地点独立成卡） |

## 改哪里（速查）

| 要改的行为 | 首选落点 |
|------------|----------|
| setup / 卷相位 / 跳章开关 | `config/features.yaml` |
| 正文硬规则词表/阈值 | `config/content_rules.yaml` |
| 系统禁名 | `config/naming_rules.yaml` |
| 章长/硬门 | `config/chapter.yaml` |
| 短剧篇幅默认 | `config/script.yaml` + `web/src/chapterTargets.js` |
| 章步骤顺序 | `config/pipeline.yaml` + 预设 `novelx-write` |
| Agent 话术 | `integrations/dsh-preset-novelx/preset/skills/**` |
| 落盘格式契约 | `deskCheck.js` + 预设 `content-formats` |
| 阅读 API / 展示 | `web/desk-server.mjs` + `deskPreview.js` / `deskDisplay.js` |
| 旧格式迁移 | `web/src/deskMigrate.js` |
| 身位板 | `web/src/deskBodyState.js` |

## 近期行为修正主题（维护对照）

| 主题 | 要点 |
|------|------|
| 写作入口 | dsh NovelX 预设；阅读台只看稿 |
| 确定性检查 | `novelx_check`：schema / 禁名 / 字数 / 相位 |
| 内容 schema 双层 | prompt 软约束 + Node `deskCheck` 硬校验；`novelx_migrate` |
| 作品中立 / 题材中立 | 框架禁专名；不预设玄幻/修仙；单测勿抄现成书 |
| 框架 vs 作品 | 预设补能力；小说只经 NovelX 落盘；禁止手改 `projects/<书>/` |

## 文档交叉引用

- 架构总览：仓库根 `README.md`
- 配置边界：`config/README.md`
- 格式：`config/skills/content-formats.md`、`config/schemas/README.md`
- 创作工作流：`.cursor/skills/create-novel/SKILL.md`
- dsh 接入：`integrations/dsh-preset-novelx/README.md`
