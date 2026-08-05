---
name: content-formats
description: >-
  正文/章纲/总纲/卷纲/剧情卡/人物·物品·地点/世界观 的双层格式契约：
  落盘便于 Agent 加载；Web 展示简洁易读。生成时必须遵守。
---

> 短剧/漫剧剧本格式见 [`content-formats-script.md`](content-formats-script.md)（`project_mode=short_drama`）。


# 内容格式契约（生成锚定）

题材中立。机器硬校验以 `novelx_pipeline::schemas` 为准；人读摘要见 `config/schemas/README.md`。

## 双层原则

| 层 | 目标 | 约定 |
|----|------|------|
| **落盘（Agent）** | 稳定、可解析、可过滤 | 固定路径 + 固定字段/H2；frontmatter 承载 status/holdings |
| **Web 阅读** | 简洁易读 | `display_*` 渲染：去 FM、节名中文化、章纲 JSON→Markdown；**勿**为好看改落盘结构 |

生成时按「落盘」格式输出；不要为 Web 另写一份。

## 总表

| 阅读页签 | 落盘路径 | Agent 加载形态 | Web 展示 |
|----------|----------|----------------|----------|
| 正文 | `chapters/NNN/draft.md` | Markdown；首行 `# 第N章 …` | 同左（归一空白） |
| 章纲 | `chapters/NNN/outline.json` | **JSON**（字段见下） | `display_chapter_outline`→中文 Markdown |
| 总纲 | `artifacts/master_outline.md` | Markdown + 固定 H2 | 节名别名归一 |
| 卷纲 | `artifacts/arc_outlines/{NN}.md` | Markdown；H1 `# 第X卷 · …` | 同左 |
| 剧情卡 | `plots/*.md` | FM + 固定 H2 | 去 FM 后正文 |
| 人物/物品/地点 | `entities/{characters\|items\|locations}/*.md` | FM（`name/status/holdings`）+ 固定 H2 | 去 FM；H2 **中文**展示 |
| 世界观 | `artifacts/bible.md` | `# 世界观` + 编号 H2 | 同左 |
| 设定缺口 | （无文件） | API `entity_gaps[]` | 列表文案 |

`story_outline.json` 仅存 acts，**不是**阅读区总纲。

---

## 1. 正文 `draft.md`

```markdown
# 第N章 标题

正文段落…
```

- 首行必须 `# 第N章 …`（阿拉伯数字；「第一章」会被归一）
- 标题行后正文须达 `config/chapter.yaml` 硬门控（默认 ≥4500 字才可发布）；创作目标 **5000–6000**（与 writer Skill 一致；勿为凑字注水）；超过 `word_hard_max`（默认 11000）HardLong 阻断，可用 `split_chapter` 拆成两章或压缩修订
- 禁止整篇包在 ` ```json `
- 除标题行外禁止「第N章」元叙述

---

## 2. 章纲 `outline.json`（只输出 JSON）

必填：

`title, pov, time_location, goal, conflict, emotion_curve, key_events[], characters[], scene_tags[], cliffhanger, lore_queries[]`

推荐（缺省 `[]`；新章纲应填写）：

`plot_includes[], plot_defers[], items[], locations[]`

- `plot_includes`：本章必须写到的剧情推进点（短句；合计估可写成 5000–6000 字；**新生成**至少 1 条）
- `plot_defers`：明确顺延后章的点；有进行中剧情卡时至少 1 条（**新生成** soft repair）
- `key_events`：至少 2 条；**新生成/修订章纲**硬上限 4（只展开 includes）；**读旧稿 / Web 原样保存**可超过 4
- `items[]` / `locations[]`：驱动设定卡加载
- 名单用规范名，与实体卡 `name` 对齐
- 防超量优先靠章纲预算；HardLong 自动拆章仅为兜底

Web 展示结构（由系统生成，勿手写第二份）：

`# 标题` → 视角/时空 → 目标/冲突/情绪 → 本章纳入/顺延后章 → 关键事件 → 出场人物/物品/地点 → 钩子

---

## 3. 总纲 `master_outline.md`

文件本身即阅读区展示内容：以 `# 总纲` 开头的纯 Markdown。

必含 H2（别名可归一）：

- `## 一句话卖点`（或 Logline）
- `## 三幕结构` 或 `## 分卷`
- `## 主角弧`
- `## 主线冲突`

禁止：寒暄套话、\`\`\` 代码块包裹、文件路径说明、`story_outline.json` / JSON 正文、预填全书「第 N 章」列表。

---

## 4. 卷纲 `arc_outlines/{NN}.md`

```markdown
# 第X卷 · 标题

## 卷定位
## 开卷状态
## 冲突升级阶梯
## 关键节点
## 人物弧
## 伏笔
## 目标章数（软）  ← 可选；如 `30–50`，非硬停
## 卷末终止条件   ← 下列表 ≥2 条
## 卷末交付
```

不定章数；终止条件可检验。`目标章数（软）` 可选，不进硬校验。

---

## 5. 剧情卡 `plots/*.md`

**FM：** `title, scope=local, plot_type, status, needs_bridge`（可选 `holdings` 无关）

**正文 H2：** `概览` · `剧情走向` · `冲突与赌注` · `出场人物` · `收束条件`

剧情卡 = 卷内一段，勿复述整卷卷纲。

---

## 6. 人物 / 物品 / 地点卡

**FM（写章过滤用）**

| 键 | 说明 |
|----|------|
| `name` | 规范名（必填） |
| `status` | `active` / `background` / `exited` / `consumed` |
| `holdings` | 人物持有物（规范名，逗号分隔）；可空 |
| `complete` | 是否补全；新建 stub 可为 `false` |
| `source` | `volume_sync` / `chapter_sync` / `plot_sync` / 手工 |

`status=exited|consumed` 默认不进 Canon（主角 always-include 例外）。

**正文必填 H2（落盘英文 canon；中文别名可写，系统归一）**

| 类型 | 落盘 H2（Agent） | Web 展示 |
|------|------------------|----------|
| 人物 | History · Personality · Core events · Current status | 经历 · 性格 · 核心事件 · 当前状态 |
| 物品 | Origin · Usage · Current status | 来源 · 用途 · 当前状态 |
| 地点 | Overview · Factions · Production | 概述 · 此地势力 · 产出资源 |

- 叙事节 `Current status` ≠ FM `status`
- **卷末同步**：直接写 FM `status`/`holdings` + 覆盖 `## 当前状态` 要点；**不要**堆「卷末同步摘要」
- **章后/剧情轻量同步**：可更新 FM；短备注可进同步节，勿冲掉设定正文

**物品录入门槛**

| 建卡 | 不建卡 |
|------|--------|
| 有实质用途/特性（改变行动、带规则/代价、可反复调用） | 一次性环境道具、普通照明/交通工具 |
| 后续剧情会再用（线索链、伏笔道具、须再核对/交易/回收） | 仅气氛描写的随身物（如走廊里随手照明的手电筒） |

细节可写进章纲/正文/地点 Overview，勿为氛围物占 `entities/items/`。

**地点层级（先母卡，有依赖则收录子区）**

| 步骤 | 规则 |
|------|------|
| 1 | 先建**母地点**卡（小区/园区/街区等，`independent=true`） |
| 2 | 再建楼栋/树阵/房间前：判断是否依赖已有母卡 |
| 3 | **有依赖** → 更新母卡：`### 子区` + `aliases`，**不**新建 `locations/*.md` |
| 4 | **独立无依赖** → 才允许新建地点卡 |

| 层级 | 例 | 落盘 |
|------|-----|------|
| 母地点 | 小区 / 园区 / 街区 / 营地 | **一张** `locations/` 卡 |
| 子区 | 楼栋、树阵、广场、楼梯、楼层、房间 | 母卡 Overview 分区 + aliases |
| 独立舞台 | 明确离开母地、长期另起主舞台 | 才可另建地点卡 |

---

## 7. 世界观 `bible.md`

```markdown
# 世界观 Bible

## 0. 一句话世界
## 1. 时代与叙事框架
## 2. 全局势力与阵营
## 3. 规则 / 能力 / 职业体系（若有）
## 4. 关键地理索引（一句话；细节进地点卡）
## 5. 历史与传说
## 6. 硬性禁忌
## 7. 开放问题
```

硬校验至少：`# 世界观…` + `## 0.` / `## 1.` / `## 2.` / `## 7.`。

全局规则进 Bible；人物/物品/地点细节进实体卡。

---

## 生成检查清单

1. 路径与类型匹配上表  
2. 必填字段/H2 齐全（缺节会被拒写）  
3. 实体名单、持有物用**规范名**  
4. 不为 Web 「美化」而改落盘结构或另存第二份  
5. 题材中立：不套默认玄幻/爽文模板  
