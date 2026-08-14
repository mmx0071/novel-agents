---
name: novelx-dev
description: >-
  NovelX / novel-agents 框架开发规范：写作在 dsh NovelX 预设，本仓库补预设/
  阅读台/Node 检查/禁名；题材中立、落盘/Web 双层与常见反模式。
  Use when editing Web UI, config, dsh preset, schemas, tests, or any
  product code in this repo. Prefer loading
  before changing prompts, defaults, fixtures, reader formats, or agent
  loop behavior. Also load when a writing-process gap appears — fix the
  preset or desk, do not hand-edit the novel.
---

# NovelX 项目开发规范

约束 **产品代码与框架配置**。创作工作流见 [create-novel](../create-novel/SKILL.md)。  
细节与反模式扩写见 [reference.md](reference.md)。

## 框架补能力，作品交给 dsh 预设落盘（硬边界）

**写作入口是 dsh 的 NovelX 预设**（不是本仓库对话，也不是 `dsh plugin add`）。  
本仓库补可复用能力：预设 Skill、阅读台、schema、禁名。情节只经预设 Agent 写入 `projects/`。

| 谁 | 做什么 | 不做什么 |
|----|--------|----------|
| dsh 预设 `novelx` | 对话写书：子 agent 改 `projects/`；`novelx_open_desk` / `novelx_check` | 不改 `config/` / `web/` |
| 本仓库（`web/**`、`config/**`、预设源、本 Skill） | 预设话术、阅读台、Node 检查、`naming_rules.yaml`、中性单测 | 不手改 `projects/<书名>/` 来「修这本书」 |
| 创作侧（create-novel） | 在 dsh 里选 NovelX 写 | 不用编辑器代替预设落盘 |

发现作品侧缺口时按这次序：

1. **判断**：预设 Skill / 阅读台不足，还是这本书没按预设步骤写？
2. **能力缺口** → 先改 `integrations/dsh-preset-novelx/preset/` 或阅读台 / schema（题材中立）→ 再开新 dsh 会话验证。
3. **禁止**直接改 `projects/<书>/entities/**`、`artifacts/bible.md`、章 `draft.md` 当修复。

## 改代码前决策树

0. **作品文件（情节 / 设定 / Bible / 章稿）** → 不改；交给 NovelX。只改框架时才继续下面步骤。
1. **检查规则 / 禁名 / 字数 / 相位** → 改 `config/content_rules.yaml`、`naming_rules.yaml`、`chapter.yaml`、`features.yaml`。
2. **结构校验 / 阅读区格式** → 改 `web/src/deskCheck.js` + `deskDisplay.js`，并同步预设 `content-formats`（必要时 `novelx_migrate`）。
3. **Agent 话术 / 编排** → 改 `integrations/dsh-preset-novelx/preset/skills/**`。
4. **章步骤** → `config/pipeline.yaml` 的 `order` 与预设 `novelx-write` 对齐。

## 题材 / 作品中立（硬规则）

与上一节分工：上一节管**谁改文件**；本节管**框架里能不能出现某本书或某种题材默认值**。

**作品中立**（不绑定某一本）：

1. 不得在 `web/**`、`config/**`、预设源写死某一作品的书名、角色、势力、地名、梗概或卷名。
2. 样例只在 `projects/<name>/`；框架经路径 / `project` 参数读写，不假设当前书。
3. 测试用中性占位：`sample-novel`、`demo`、`主角`、`甲`、`信物`、`样例小区`；情节用抽象句。
4. 单测/夹具禁止从 `projects/` 现成书抄专名、章题、剧情卡标题或段落。
5. 集成/迁移测试勿绑死 `projects/某书名`；优先临时目录最小骨架；可选扫 `projects/` 须枚举且失败 skip。
6. 某书的写作事故（漏同步、地点被 alias 吞掉、伤势未回写）→ 抽成通用规则，禁止把该书专名写进前端 / 检查工具特判。

**题材中立**（不预设网文模板）：

7. Prompt / Skill 不预设玄幻/修仙/爽文默认世界观；由该项目 brief + Bible 决定。
8. `body_state` / 硬规则只抓结构 cue（侧别、载体、H2 名等），不写故事专名。

违规：改为通用占位或下沉 `projects/`，并补回归证明框架不依赖该书名。  
修复作品缺口：改预设或检查工具 → 再开 NovelX 会话。

## Prompt vs Schema vs 确定性检查

| 层 | 权威 | 用途 |
|----|------|------|
| Prompt / Skill | `integrations/dsh-preset-novelx/preset/skills/**` | 软约束：写章目标、编排、Agent 行为 |
| Schema | `web/src/deskCheck.js` + `deskDisplay.js` | 结构硬校验；人读摘要在预设 `content-formats` |
| 确定性检查 | `novelx_check` 读 `content_rules` / `chapter.yaml` / `features.yaml` | 禁名、字数、setup/volume 相位、跳章 |

- `content_rules`：`enabled=false` 跳过；`blocking=false` 仅警告。词表/阈值/文案在 YAML。
- 阅读台只读：`PUT /content` 拒绝写入，改稿在 dsh。

## 配置（优先于硬编码）

- **布尔行为** → `config/features.yaml`（setup / 卷相位 / 跳章）。
- **字数** → `chapter.yaml`（长篇）/ `script.yaml`（短剧，与 `chapterTargets.js` 对齐）。
- **禁名 / 硬规则** → `naming_rules.yaml` / `content_rules.yaml`。
- **章步骤** → `pipeline.yaml`。
- API Key 由 dsh 管理，不进本仓库配置。

## Node 落点

| 文件 | 职责 |
|------|------|
| `web/desk-server.mjs` | 阅读台 HTTP / SSE |
| `web/src/deskPreview.js` | library / preview |
| `web/src/deskCheck.js` | schema / 禁名 / 字数 / 相位 |
| `web/src/deskMigrate.js` | 旧格式迁移 |
| `web/src/deskBodyState.js` | 身位板（读各章 `summary.json`） |
| `integrations/dsh-preset-novelx/preset/` | 写作 Skill 与工具 |

## 检查闸（实现时勿绕过）

对应 `features.yaml`（默认 true）：

- `enforce_setup_gate`：`setup_phase != ready` 硬拦写新章
- `enforce_volume_phase`：非 `drafting_volume` 拦写新章
- `enforce_chapter_order`：跳章 / 前章缺口硬拦

写设定或写章后先 `novelx_check`。有 blocker 不要推进 `next_chapter`。

## 超长篇约束（800–1000 章 × 5–6k）

- 字数：目标 5000–6000；硬门默认 ≥4500。
- 一致性 P0 **挡推进**；节奏问题不挡。
- `plot_acceptor` 未收束不挡本章推进（跨多章铺垫）；`pass=true` 才把剧情卡标 `completed`。
- Web 章列表：preview 只带 `has_draft`/`body_chars`；正文懒加载。

## 内容双层（落盘 vs Web）

- 生成只写**落盘**形态；Web 用展示层渲染，**勿为好看改落盘或另存第二份**。
- 章纲只写 `outline.json`。
- 实体：落盘 FM + 英文 H2 canon；展示去 FM、H2 中文；同步写 `status`/`holdings` + `## 当前状态`，不堆「卷末同步摘要」。
- `story_outline.json` 仅存 acts，不是阅读区总纲。
- 改格式须同步预设 `content-formats` + `novelx_migrate`。

## 测试与迁移

- `novelx_migrate`：阅读区格式迁移（outline.md→json 等），**不发明情节**。
- 检查单测在 `web/src/deskCheck.test.js` 等；改 schema 同步 `content-formats`。
- 发现违规中立：改通用实现并补回归。

## 常见反模式（近期修正沉淀）

完整列表见 [reference.md](reference.md#常见反模式)。开发时至少避开：

1. 在前端 / 检查工具硬编码某书专名或题材模板
2. 为 Web 手写第二份章纲 Markdown / 改落盘结构
3. 手改 `projects/<书>/` 来修同步/设定缺口（应改预设再开 NovelX）
4. 单测夹具抄现成书的书名、角色、章题或剧情卡标题
5. 有 blocker 仍推进 `next_chapter`
6. 把一张剧情卡当整卷；或 `plot_acceptor` 改写收束条件来 pass

## 与其它规范的关系

| 主题 | 权威 |
|------|------|
| 落盘/展示 | `config/skills/content-formats.md`、`config/schemas/README.md` |
| 章步骤 | `config/pipeline.yaml` + 预设 `novelx-write` |
| 配置编辑边界 | `config/README.md` |
| 架构总览 | `README.md` |
| 创作工作流 | `.cursor/skills/create-novel/SKILL.md`（dsh 预设） |
| dsh 接入 | `integrations/dsh-preset-novelx/README.md` |

## 项目模式（longform / short_drama）

- `meta.json` → `project_mode`：`longform`（默认）或 `short_drama`。
- 短剧落盘 `episodes/NNN/script.md`；**勿**套用 `chapter.yaml` 4500 字门。
