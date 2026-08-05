---
name: content-formats-script
description: AI 漫剧短篇（project_mode=short_drama）落盘与阅读区格式契约。
---

# 短剧 / 漫剧剧本格式

适用于 `meta.json` 中 `project_mode: short_drama` 的项目。超长篇章格式见 `content-formats.md`。

## 目录

```
projects/<name>/
  meta.json                 # project_mode: short_drama
  episodes/NNN/
    outline.json            # 集纲
    script.md               # 剧本正文
  entities/…                # 与长篇相同
  plots/…                   # 作 beat 卡（冲突单元），非卷内长篇剧情卡语义
  artifacts/
    series_outline.md       # 系列/短剧总纲（亦接受 master_outline.md）
    bible.md
```

## 集纲 `outline.json`

字段与章纲兼容（便于复用 planner 校验），语义映射：

| 字段 | 短剧含义 |
|------|----------|
| title | 集标题 |
| pov | 主视角角色 |
| time_location | 本集时空跨度 |
| goal / conflict | 本集目标与冲突 |
| plot_includes | 必须兑现的 beat（短句） |
| key_events | 场次节拍（可多于长篇 4 条） |
| cliffhanger | 集末钩子 |
| scene_tags | 如 dialogue_heavy / twist / climax |

另可在 JSON 中带 `target_duration_sec`（提示用，不挡发布）。

## 剧本 `script.md`

```markdown
# 第N集 · 标题
目标时长：约 Xs

## 场1 · 地点 / 日夜
【画面】一句话可见画面（构图/动作/情绪）
角色A：对白
角色B：对白
OS 角色A：内心独白（可选）
VO：旁白（可选）

## 场2 · …
…

【钩子】下一集必须接住的悬念一句
```

硬规则：

1. 首行必须是 `# 第N集 …`（N 与目录号一致）
2. 至少 2 个 `## 场` 节
3. 必须有 `【钩子】` 行（可在文末）
4. 正文（标题行后）字数门见 `config/script.yaml`，**不是** chapter.yaml 的 4500+

## 系列总纲（`series_outline.md` / `master_outline.md`）

短剧 `design_master_outline` 必填 H2：

- `## 一句话卖点`
- `## 分集骨架`（或 `## 分集` / `## 分季`；亦接受长篇的 `## 分卷` / `## 三幕结构`）
- `## 主角弧`
- `## 主线冲突`

## 与长篇差异（勿混用）

- 不用 `chapters/`、`volume_phase`、`sync_volume`、`continue_writing_batch`、`split_chapter`
- 定稿不要求卷纲；总纲用系列大纲即可
- Web 阅读区展示 `script.md`，不是长篇散文 draft
