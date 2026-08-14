#!/usr/bin/env bash
# 一键：挂载 NovelX 预设、重启阅读台、启动 dsh Web
#
# 用法：
#   ./scripts/restart.sh              # 构建前端后后台启动阅读台 + dsh
#   ./scripts/restart.sh --quick      # 不重新 npm build
#   ./scripts/restart.sh --bind 0.0.0.0:8765
#   ./scripts/restart.sh --foreground # 阅读台后台，dsh 前台（Ctrl+C 停 dsh）
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BIND="${NOVELX_BIND:-127.0.0.1:8765}"
PORT="${BIND##*:}"
DSH_BIND="${DSH_BIND:-127.0.0.1:3080}"
DSH_PORT="${DSH_BIND##*:}"
DSH_HOME="${DSH_HOME:-$HOME/.dsh}"
BUILD_WEB=1
DAEMON=1

usage() {
  cat <<'EOF'
一键挂载 NovelX、重启阅读台、启动 dsh

用法：
  ./scripts/restart.sh              # 构建前端后后台启动阅读台 + dsh
  ./scripts/restart.sh --quick      # 不重新 npm build
  ./scripts/restart.sh --bind 0.0.0.0:8765
  ./scripts/restart.sh --foreground # 阅读台后台，dsh 前台
EOF
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage ;;
    --quick|--no-web|--no-build) BUILD_WEB=0; shift ;;
    --release) shift ;;
    --daemon|-d) DAEMON=1; shift ;;
    --foreground|-f) DAEMON=0; shift ;;
    --bind) BIND="$2"; PORT="${BIND##*:}"; shift 2 ;;
    --bind=*) BIND="${1#*=}"; PORT="${BIND##*:}"; shift ;;
    *)
      echo "未知参数: $1（用 --help 查看用法）" >&2
      exit 2
      ;;
  esac
done

kill_port() {
  local p="$1"
  local pids
  pids="$(lsof -tiTCP:"$p" -sTCP:LISTEN 2>/dev/null || true)"
  if [[ -n "${pids}" ]]; then
    echo "→ 停止占用 :$p 的进程: $pids"
    # shellcheck disable=SC2086
    kill $pids 2>/dev/null || true
    sleep 0.4
    pids="$(lsof -tiTCP:"$p" -sTCP:LISTEN 2>/dev/null || true)"
    if [[ -n "${pids}" ]]; then
      # shellcheck disable=SC2086
      kill -9 $pids 2>/dev/null || true
      sleep 0.2
    fi
  else
    echo "→ :$p 无监听进程"
  fi
}

port_up() {
  local p="$1"
  [[ -n "$(lsof -tiTCP:"$p" -sTCP:LISTEN 2>/dev/null || true)" ]]
}

wait_port() {
  local p="$1"
  local n="${2:-80}"
  local i
  for ((i = 0; i < n; i++)); do
    if port_up "$p"; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

print_urls() {
  echo
  echo "打开："
  echo "  阅读台  http://${BIND}/"
  if port_up "$DSH_PORT"; then
    echo "  dsh     http://${DSH_BIND}/   （新开会话，预设 NovelX）"
  else
    echo "  dsh     http://${DSH_BIND}/   （未起来，见 .novelx/dsh.log）"
  fi
}

resolve_dsh_root() {
  if [[ -n "${DSH_ROOT:-}" && -f "$DSH_ROOT/package.json" ]]; then
    printf '%s' "$DSH_ROOT"
    return
  fi
  if [[ -f "$ROOT/../deepseek-harness/package.json" ]]; then
    (cd "$ROOT/../deepseek-harness" && pwd)
    return
  fi
  return 1
}

run_dsh() {
  cd "$ROOT"
  if [[ -n "${DSH_BIN:-}" ]]; then
    "$DSH_BIN" "$@"
    return
  fi
  if command -v dsh >/dev/null 2>&1; then
    dsh "$@"
    return
  fi
  # 旁路源码仅在显式 DSH_ROOT 时用；邻目录 deepseek-harness 可能未编过，会直接炸。
  if [[ -n "${DSH_ROOT:-}" ]]; then
    local src
    if src="$(resolve_dsh_root)"; then
      local tsx="$src/node_modules/.bin/tsx"
      if [[ -x "$tsx" && -f "$src/apps/cli/src/bin.ts" ]]; then
        "$tsx" "$src/apps/cli/src/bin.ts" "$@"
        return
      fi
    fi
  fi
  npx --yes @deepseek-ai/dsh "$@"
}

install_preset() {
  local src="$ROOT/integrations/dsh-preset-novelx/preset"
  local dest="$DSH_HOME/.agent-presets/novelx"
  if [[ ! -f "$src/preset.yml" ]]; then
    echo "错误: 找不到预设 $src/preset.yml" >&2
    exit 1
  fi
  mkdir -p "$dest"
  echo "→ 挂载 NovelX 预设 → $dest"
  rsync -a --delete "$src/" "$dest/"
}

ensure_default_preset() {
  local f="$DSH_HOME/settings.yaml"
  mkdir -p "$DSH_HOME"
  python3 - "$f" <<'PY'
from pathlib import Path
import re
import sys

path = Path(sys.argv[1])
text = path.read_text() if path.exists() else ""
block = re.search(r"(?ms)^agent-presets:\n(?:[ \t].*\n)*", text)
if block and re.search(r"(?m)^[ \t]+default:\s*novelx\s*$", block.group(0)):
    sys.exit(0)
if block:
    body = block.group(0)
    if re.search(r"(?m)^[ \t]+default:", body):
        body = re.sub(r"(?m)^([ \t]+)default:.*$", r"\1default: novelx", body, count=1)
    else:
        body = "agent-presets:\n  default: novelx\n" + body[len("agent-presets:\n"):]
    text = text[: block.start()] + body + text[block.end():]
else:
    if text and not text.endswith("\n"):
        text += "\n"
    text += "\nagent-presets:\n  default: novelx\n"
path.write_text(text)
PY
  echo "→ dsh 默认预设: novelx"
}

echo "== NovelX 重启 =="
echo "  root:   $ROOT"
echo "  阅读台: $BIND"
echo "  dsh:    $DSH_BIND"

install_preset
ensure_default_preset

if [[ "$BUILD_WEB" -eq 1 ]]; then
  echo "→ npm run build (web/)"
  if [[ ! -d "$ROOT/web/node_modules" ]]; then
    (cd "$ROOT/web" && npm install)
  fi
  (cd "$ROOT/web" && npm run build)
else
  echo "→ 跳过前端构建"
fi

if [[ ! -f "$ROOT/web/desk-server.mjs" ]]; then
  echo "错误: 找不到 $ROOT/web/desk-server.mjs" >&2
  exit 1
fi

mkdir -p "$ROOT/.novelx"
LOG="$ROOT/.novelx/web.log"
PID_FILE="$ROOT/.novelx/web.pid"
DSH_LOG="$ROOT/.novelx/dsh.log"
DSH_PID_FILE="$ROOT/.novelx/dsh.pid"

echo "→ 切换阅读台 :$PORT"
kill_port "$PORT"
echo "→ 启动: node web/desk-server.mjs --bind $BIND"
nohup env NOVELX_ROOT="$ROOT" NOVELX_BIND="$BIND" node "$ROOT/web/desk-server.mjs" --bind "$BIND" >"$LOG" 2>&1 &
echo $! >"$PID_FILE"
if ! wait_port "$PORT" 20; then
  echo "阅读台启动失败，见日志: $LOG" >&2
  tail -n 40 "$LOG" >&2 || true
  exit 1
fi
echo "✓ 阅读台 pid=$(cat "$PID_FILE")"

echo "→ 切换 dsh :$DSH_PORT"
kill_port "$DSH_PORT"
echo "→ 启动: dsh web --port ${DSH_PORT}  (cwd=${ROOT}, preset=novelx)"
export NOVELX_ROOT="$ROOT" NOVELX_BIND="$BIND"
if [[ "$DAEMON" -eq 1 ]]; then
  (
    cd "$ROOT"
    run_dsh web --port "$DSH_PORT"
  ) >"$DSH_LOG" 2>&1 &
  echo $! >"$DSH_PID_FILE"
  if ! wait_port "$DSH_PORT" 120; then
    echo "dsh 启动失败，见日志: $DSH_LOG" >&2
    tail -n 40 "$DSH_LOG" >&2 || true
    exit 1
  fi
  echo "✓ dsh pid=$(cat "$DSH_PID_FILE")"
  echo "  日志: $LOG"
  echo "  dsh 日志: $DSH_LOG"
  print_urls
else
  print_urls
  echo "✓ dsh 前台运行（Ctrl+C 停止）"
  run_dsh web --port "$DSH_PORT"
fi
