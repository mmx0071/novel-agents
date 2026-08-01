# 配置目录说明

本目录存放 **框架级** 配置与 Skills。Web「配置」页可在线编辑其中一部分；保存后热重载，无需重启服务。

## 可编辑配置 vs Rust 引擎

| 文件 / 路径 | 谁改 | 引擎职责 |
|-------------|------|----------|
| `content_rules.yaml` | Web「硬规则」或手工编辑 | Rust 只做通用扫描；`enabled=false` 跳过扫描；`blocking=false` 仅警告不挡发布；词表 / 阈值 / 文案在 YAML |
| `naming_rules.yaml` | Web「禁名」 | 禁名匹配与取名原则注入 prompt |
| `policies.yaml` | HTTP API / 手工（Web ConfigPanel 无 Tab） | Studio 意图短语、扩写判定等；PUT 后内存热重载 |
| `skills/**` | Web「Skills」（仅框架 skill） | Agent 提示正文；PUT 后 `reload_skills` |
| `llm.yaml` | Web「模型」结构化表单或手工编辑 | Provider / 任务模型 / profile / 重试；PUT 后热重载 |
| `gates.yaml` / `features.yaml` / `intents.yaml` | 磁盘编辑（本期无表单） | 门控、特性开关（含 `pipeline.auto_split_hard_long`）、意图路由 |
| `mutation_policy.yaml` | 磁盘编辑 | 改盘严重度：routine 自确认 / high 人审；低风险 impact 自动级联；配合 `studio.mutation_severity_policy` |
| `decision_council.yaml` | 磁盘编辑 | 评审团自动决策、章边界密封、按需素材 Agent；配合 `studio.decision_council` / `studio.seal_on_chapter_pass`；`chapter_next_clean` 干净发布自动续写 |
| `chapter.yaml` | 磁盘编辑 | 章长目标 / 软硬上下限（`word_hard_max`→HardLong 可拆章）/ 连续偏短升格 |
| `longform.yaml` | 磁盘编辑 | 超长篇：`quality_tier` / `audit_tier` / `impact_scan_mode` / `batch_max_chapters`（成功发布上限）/ `foreshadow_debt`（近债分级：宽限与远期不挡批写）；`audit_tier: layered` 常规章用轻量一致性上下文（高潮/奇数章/复审仍 full），省 token，偶发漏检风险略高于 `full` |
| `continuity.yaml` / `volume.yaml` | 磁盘编辑 | CanonContext 预算、薄卷阈值 |

API Key 只写入仓库根 `.env`（gitignore），**不进** `llm.yaml`。`GET /api/config/llm` 只返回 `has_api_key` 与末 4 位 suffix，永不回传明文 Key；PUT 时 `api_key` 留空表示不修改。

## API（摘要）

- `GET/PUT /api/config/content_rules` · `PUT /api/config/content_rules/flags`（切换单条 enabled/blocking）
- `GET/PUT /api/config/naming_rules`
- `GET/PUT /api/config/policies`（HTTP/手工；Web ConfigPanel 无 Tab）
- `GET/PUT /api/config/llm`（结构化 JSON；Key 只写不读）
- `GET /api/skills/list` · `GET/PUT /api/skills/{name}`（PUT 拒绝 `projects/` 下的 project-skill）

路径均限制在 `config/` 下，防止目录穿越。

### CLI / API-only（Web 未接 UI）

下列路由保留给脚本与 HTTP 客户端，**不删实现**；当前 Web 前端不调用：

| 路由 | 说明 |
|------|------|
| `GET/PUT /api/config/policies` | 改 `policies.yaml`；Web 配置页无 policies Tab |
| `GET /api/projects/{name}/ops_journal` | 操作审计查询；主入口为 CLI `novel ops-log` |
| `GET /api/projects/{name}/version_nodes` | shadow git 版本节点列表；CLI `novel versions list` |
| `POST /api/projects/{name}/version_nodes/{sha}/restore` | 回退工作树；CLI `novel versions restore <sha>`（高位，须确认） |
| `POST /api/thread/resume` | HTTP 恢复线程；Web 走 `POST /api/thread/start` |
| `POST /api/turn/steer` | HTTP mid-turn steer；Web 走 WS + 工具 `steer_run` |

## 与作品数据的边界

具体小说内容只在 `projects/<name>/`。本目录保持题材中立，不写死某一本书的角色或地名。

## 操作审计 vs 内容审校

| 概念 | 路径 / 开关 | 用途 |
|------|-------------|------|
| **ops journal**（决策/执行审计） | `projects/<name>/.novelx/ops_journal.jsonl`；`studio.ops_journal` | 回放工具、门控、mutation、发布等「发生了什么」；append-only，清聊天不删。`audit_report` kind ≠ `publish_result` |
| **version nodes**（内容回退） | `projects/<name>/.novelx/versions.git` + `version_nodes.jsonl`；`studio.version_nodes` | Cursor/Claude 式本地 shadow git；改盘前 / 章通过 / 剧情完结 / 卷 sync 前打点；可 list + restore |
| **consistency audit**（内容审校） | `chapters/NNN/audit.json`、`audit_queue`；`audit_tier` | 正文一致性检查结果，不是系统操作日志 |

查询：`novel ops-log <project>` 或 `GET /api/projects/{name}/ops_journal`。  
版本：`novel versions <project> list` / `novel versions <project> restore <sha>`。

### 自确认（1C）

- `studio.require_mutation_confirm` + `studio.mutation_severity_policy`：仅 `mutation_policy.yaml` 中 **high_tools** 弹人审；routine 工具系统自签发落盘。
- 仍人审：真 P0 审校、Setup 定稿、卷交接、设定 BLOCKER、删实体 / 总纲卷纲 / Bible upsert、高风险 impact。
- 低风险 impact：命中数 ≤ `impact.auto_max_hits` 且无 draft 目标、源非 bible/master_outline → 自动级联。
- `decision_council.chapter_next_clean`：干净发布后自动续写（`config/decision_council.yaml` 现为 `enabled: true`；缺省文件时 Rust defaults 仍为 false）。
