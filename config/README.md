# 配置目录说明

本目录存放 **框架级** 配置与 Skills。Web「配置」页可在线编辑其中一部分；保存后热重载，无需重启服务。

## 可编辑配置 vs Rust 引擎

| 文件 / 路径 | 谁改 | 引擎职责 |
|-------------|------|----------|
| `content_rules.yaml` | Web「硬规则」或手工编辑 | Rust 只做通用扫描；`enabled=false` 跳过扫描；`blocking=false` 仅警告不挡发布；词表 / 阈值 / 文案在 YAML |
| `naming_rules.yaml` | Web「禁名」 | 禁名匹配与取名原则注入 prompt |
| `policies.yaml` | Web API / 手工 | Studio 意图短语、扩写判定等；PUT 后内存热重载 |
| `skills/**` | Web「Skills」（仅框架 skill） | Agent 提示正文；PUT 后 `reload_skills` |
| `llm.yaml` | Web「模型」结构化表单或手工编辑 | Provider / 任务模型 / profile / 重试；PUT 后热重载 |
| `gates.yaml` / `features.yaml` / `intents.yaml` | 磁盘编辑（本期无表单） | 门控、特性开关、意图路由 |
| `chapter.yaml` | 磁盘编辑 | 章长目标 / 硬门 / 连续偏短升格 |
| `longform.yaml` | 磁盘编辑 | 超长篇：`quality_tier` / `audit_tier` / `impact_scan_mode` / `batch_max_chapters`；`audit_tier: layered` 常规章用轻量一致性上下文（高潮/奇数章/复审仍 full），省 token，偶发漏检风险略高于 `full` |
| `continuity.yaml` / `volume.yaml` | 磁盘编辑 | CanonContext 预算、薄卷阈值 |

API Key 只写入仓库根 `.env`（gitignore），**不进** `llm.yaml`。`GET /api/config/llm` 只返回 `has_api_key` 与末 4 位 suffix，永不回传明文 Key；PUT 时 `api_key` 留空表示不修改。

## API（摘要）

- `GET/PUT /api/config/content_rules` · `PUT /api/config/content_rules/flags`（切换单条 enabled/blocking）
- `GET/PUT /api/config/naming_rules`
- `GET/PUT /api/config/policies`
- `GET/PUT /api/config/llm`（结构化 JSON；Key 只写不读）
- `GET /api/skills/list` · `GET/PUT /api/skills/{name}`（PUT 拒绝 `projects/` 下的 project-skill）

路径均限制在 `config/` 下，防止目录穿越。

## 与作品数据的边界

具体小说内容只在 `projects/<name>/`。本目录保持题材中立，不写死某一本书的角色或地名。
