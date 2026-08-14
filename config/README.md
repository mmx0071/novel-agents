# 配置目录说明

本目录是 **框架级** 旋钮。写作在 dsh NovelX 预设里完成；`novelx_check` 读这里的检查规则。接入见 [dsh 预设](../integrations/dsh-preset-novelx/README.md)。

具体小说只在 `projects/<name>/`，由预设 Agent 落盘。本目录保持作品中立 + 题材中立。

## 仍会读的文件

| 文件 | 谁读 | 作用 |
|------|------|------|
| `naming_rules.yaml` | `novelx_check` + 预设 Skill | 系统禁名 |
| `chapter.yaml` | `novelx_check` | 长篇章长目标与字数硬门 |
| `script.yaml` | 阅读台字数条（短剧） | 短剧篇幅默认，与 `chapterTargets.js` 对齐 |
| `content_rules.yaml` | `novelx_check` | 正文硬规则；`enabled=false` 跳过；`blocking=false` 仅警告 |
| `features.yaml` | `novelx_check` | setup / 卷相位 / 跳章开关 |
| `pipeline.yaml` | 文档 + `novelx_write` | 章步骤顺序 |
| `skills/content-formats.md` | 人读契约 | 落盘/展示格式；生成锚定以预设 `content-formats` 为准 |
| `skills/content-formats-script.md` | 人读契约 | 短剧剧本格式 |
| `schemas/README.md` | 人读摘要 | 各页签路径与字段 |

模型与 API Key 由 **dsh** 自己管，不进本目录。

## 与作品的边界

不要手改 `projects/<书>/` 来修某本书。能力缺口改预设 Skill 或 `web/src/deskCheck.js` / `deskMigrate.js`。
