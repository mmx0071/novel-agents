# 配置目录说明

本目录存放 **框架级** 配置与 Skills。Web「配置」页可在线编辑其中一部分；保存后热重载，无需重启服务。

## 可编辑配置 vs Rust 引擎

| 文件 / 路径 | 谁改 | 引擎职责 |
|-------------|------|----------|
| `content_rules.yaml` | Web「硬规则」或手工编辑 | Rust 只做通用扫描（行扫描、引号剥离、单调比较）；词表 / 阈值 / 开关 / 文案模板在 YAML |
| `naming_rules.yaml` | Web「禁名」 | 禁名匹配与取名原则注入 prompt |
| `policies.yaml` | Web API / 手工 | Studio 意图短语、扩写判定等；PUT 后内存热重载 |
| `skills/**` | Web「Skills」（仅框架 skill） | Agent 提示正文；PUT 后 `reload_skills` |
| `gates.yaml` / `features.yaml` / `intents.yaml` / `llm.yaml` | 磁盘编辑（本期无表单） | 门控、特性开关、意图路由、LLM |

## API（摘要）

- `GET/PUT /api/config/content_rules`
- `GET/PUT /api/config/naming_rules`
- `GET/PUT /api/config/policies`
- `GET /api/skills/list` · `GET/PUT /api/skills/{name}`（PUT 拒绝 `projects/` 下的 project-skill）

路径均限制在 `config/` 下，防止目录穿越。

## 与作品数据的边界

具体小说内容只在 `projects/<name>/`。本目录保持题材中立，不写死某一本书的角色或地名。
