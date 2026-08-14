---
name: content-formats
description: 作品落盘路径与字段。写 projects/ 前必须加载。题材中立，不写某书专名。
---

# 落盘格式（dsh 路径）

工作区是仓库根。作品只写 `projects/<目录>/`。`<目录>` 由用户或会话约定，禁止写死书名。

| 产物 | 路径 | 形态 |
|------|------|------|
| 进度 | `state.json` | JSON；关注 `next_chapter`、`published_count` |
| 正文 | `chapters/NNN/draft.md` | 首行 `# 第N章 标题`，段间空一行 |
| 章纲 | `chapters/NNN/outline.json` | 只 JSON，见下 |
| 摘要 | `chapters/NNN/summary.json` | JSON |
| 审校 | `chapters/NNN/audit.json` | JSON |
| 剧情验收 | `chapters/NNN/plot_accept.json` | JSON |
| 总纲 | `artifacts/master_outline.md` | `# 总纲` + 固定 H2 |
| 卷纲 | `artifacts/arc_outlines/{NN}.md` | `# 第X卷 · …` |
| 世界观 | `artifacts/bible.md` | `# 世界观` |
| 剧情卡 | `plots/*.md` | FM + H2 |
| 人物/物品/地点 | `entities/{characters\|items\|locations}/*.md` | FM + H2 |

禁止改 `config/`、`web/`。禁止调用 Host / `continue_writing`。落盘后调 `novelx_check`。旧格式调 `novelx_migrate`。看稿用 `novelx_open_desk`（阅读台 8765），不要把正文贴进聊天。

## 禁名（取名 / 写章 / 审校前必读）

两层合并，命中则换名，不要沿用：

| 层 | 路径 | 谁改 |
|----|------|------|
| 系统（只读） | `config/naming_rules.yaml` → `forbidden_names`、`naming_principles` | 框架；写作 Agent **禁止**改这个文件 |
| 作品（可写） | `projects/<目录>/lore/nomenclature.json` → `forbidden_names` | 本作品加码；没有该字段当空 |

写新名或扫正文前先 `read` 这两处。系统清单漏掉的语料套路名：记进该作品 `forbidden_names`，并告诉用户可补进 `config/naming_rules.yaml`（你不要代改）。

作品目录改名或复制存档用 `bash`：`mv` / `cp -R`，范围仅 `projects/<目录>/`。不要用 `read`+`write` 逐文件搬。目标路径已存在则先停下来问用户。

## 章纲 JSON 必填

`title, pov, time_location, goal, conflict, emotion_curve, key_events, characters, scene_tags, cliffhanger, lore_queries, plot_includes, plot_defers, items, locations`

- `key_events`：2–4 条短句
- `plot_includes`：本章必写推进点
- 正文目标 5000–6000 字；不要为凑字注水

## 剧情卡

FM：`title, scope=local, plot_type, status, needs_bridge`  
H2：`概览` · `剧情走向` · `冲突与赌注` · `出场人物` · `收束条件`

`status`：`planned` / `in_progress` / `completed`

## 实体卡

FM：`name`（规范名）、`status`（`active` / `background` / `exited` / `consumed`）、`holdings`  
人物 H2：`History` · `Personality` · `Core events` · `Current status`  
物品 H2：`Origin` · `Usage` · `Current status`  
地点 H2：`Overview` · `Factions` · `Production`
