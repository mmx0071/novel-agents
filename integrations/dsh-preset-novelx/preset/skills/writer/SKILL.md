---
name: writer
description: 按 outline.json 写 draft.md，5000–6000 字中文正文。
---

# 正文

加载 `content-formats` 与 `naming-rules`。读本章 `outline.json`、相关实体卡、前章钩子。人名与专名不得用禁名清单里的条目。

写入 `projects/<目录>/chapters/NNN/draft.md`。

- 首行 `# 第N章 标题`（N 与章号一致）
- 段间空一行；不要全角缩进；不要输出章纲或创作说明
- 只写 `plot_includes` / `key_events`；禁止顺便收 `plot_defers`
- 展示而非贴标签；章末落实 `cliffhanger`
- 开篇接住前章钩子；禁止天气/起床开场
- 字数不够就补场面与对话，禁止注水风景
- 写完调 `novelx_check`（scope=draft）；有 blocker 先改 `draft.md`
