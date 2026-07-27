---
name: novel-draft
description: >-
  立项字段约定（供 Studio 理解用户补充）。创建由 create_novel/init_novel 工具完成，
  本 Skill 不是独立提取器 LLM。
---

# 立项字段约定（NovelX）

立项与补字段走工具 / Web，**不要**假装输出 `is_novel_draft` JSON 来建库。

| 字段 | 说明 |
|------|------|
| brief | 创作设想/题材/世界观（尽量保留用户原意） |
| title | 书名 |
| project_id | `projects/` 下目录名；未指定时跟书名 |
| expected_volumes | 可选预计卷数；不强制 |
| genre | 题材标签；无法判断则「未定」 |
| mode | `outline_only`（仅大纲）或 `full`（大纲+正文） |
| through_chapter | mode=full 时**先写**到第几章（默认 1）；不是全书目标章数 |

**不要**再向用户追问全书规划章数 `target_chapters`。大纲按卷推进，正文章号随写作递增。CLI `init --chapters` 仅为 state 软上限/占位，不驱动流水线。

## Studio 理解用户补充时

- 只更新用户本轮明确提到的字段；未提及的保留已有值
- 用户指定新书名 → `project_id` 跟书名，勿沿用左侧旧项目
- 「三卷左右」→ expected_volumes；「仅大纲」→ outline_only；「先写三章」→ through_chapter=3
- 工具顺序：`create_novel` / `init_novel` → `lock_brief` → 总纲/卷纲 → Bible → `confirm_setup`
