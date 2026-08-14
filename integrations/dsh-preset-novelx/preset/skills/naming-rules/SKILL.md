---
name: naming-rules
description: 禁名两层清单。取名、写章、审校前加载。
---

# 禁名

1. `read` `config/naming_rules.yaml`（系统，只读，禁止改）。
2. `read` `projects/<目录>/lore/nomenclature.json` 的 `forbidden_names`（作品加码；没有当空）。
3. 新名、正文、章纲人物名单不得命中任一层，也避开明显变体。
4. 漏网的语料套路名写入该作品 `forbidden_names`，不要改 `config/`。
