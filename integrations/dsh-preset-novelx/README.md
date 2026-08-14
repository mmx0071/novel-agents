# NovelX × dsh 接入

NovelX 走 dsh 官方的 **agent preset**（会话级写作组合），不走 `dsh plugin add` 那种 Host 插件。

官方两套东西不要混：

| | Agent 预设 | Host 插件 / bundle |
|--|-----------|-------------------|
| 装到哪 | `~/.dsh/.agent-presets/<id>/` | `dsh plugin --profile web add <路径>` |
| 作用范围 | **每个新会话**可选 | 改整个 profile 的全局行为 |
| NovelX | 用这个 | **不要用** |

预设里可以挂相对路径插件（官方 `coding` 预设就是 `name: ./plugin.ts`）。NovelX 的阅读台是 `name: ./desk.ts`，合法，不是另装一个 bundle。

当前 dsh **没有** `dsh preset add` 命令。用户预设就是把目录放到 `~/.dsh/.agent-presets/`。本仓库用复制 / rsync 装进去。

---

## 一次性安装

在 **novel-agents 仓库根**执行：

```bash
# 1. 阅读台前端（Node；desk.ts 会起 web/desk-server.mjs，不编 Rust）
cd web && npm install && npm run build && cd ..

# 2. 装到 dsh 官方用户预设目录
mkdir -p ~/.dsh/.agent-presets/novelx
rsync -a --delete integrations/dsh-preset-novelx/preset/ ~/.dsh/.agent-presets/novelx/
```

装完后目录应是：

```
~/.dsh/.agent-presets/novelx/
  preset.yml
  agent.cordis.yml
  desk.ts
  skills/
```

可选：默认就用 NovelX（改 `~/.dsh/settings.yaml`）：

```yaml
agent-presets:
  default: novelx
```

不设也能在每个新会话的下拉里选。

开发期改了 `integrations/dsh-preset-novelx/preset/` 后，跑 `./scripts/restart.sh --quick`（会 rsync 预设并重启 dsh），然后 **新开一个会话**（已打开的会话不会重载预设）。

---

## 每次写作

1. 工作区必须是 **novel-agents 仓库根**（Agent 直接改 `projects/`，选错目录会写到别处）。
2. 启动 dsh Web：

```bash
# 已发布包
npx @deepseek-ai/dsh web

# 或本机源码（deepseek-harness 仓库）
pnpm dsh web
```

3. 浏览器打开提示的地址（常见 `http://127.0.0.1:3080`）。
4. **新开会话** → 预设选 **NovelX**（不要用标准 / coding 写书）。
5. 点名 `projects/<目录>/`。写设定或写章时 Agent 会调 `novelx_check`（schema / 禁名 / 字数 / 相位），再调 `novelx_open_desk` 拉起阅读台 `http://127.0.0.1:8765`。

主 agent 只路由：`novelx_progress` / `novelx_canon` / `novelx_write`。写章再按 `pipeline.yaml` 的顺序委派章步骤子 agent。设定与正文用文件工具改 `projects/`。

---

## 不要用的路径

- `dsh plugin --profile web add …`：Host bundle，和写作入口无关。
- 标准模式 / coding 预设写书：没有 NovelX Skill 和阅读台跟随。
- 本仓库不再提供 Studio / Host `continue_writing`。

格式靠预设 Skill；落盘后用 `novelx_check`。禁名靠 Skill 读 `config/naming_rules.yaml`（只读）和该作品 `lore/nomenclature.json` 的 `forbidden_names`。
