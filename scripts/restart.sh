#!/usr/bin/env bash
# 一键重启 NovelX（Rust web 服务，默认托管 web/dist）
#
# 用法：
#   ./scripts/restart.sh              # 编译 CLI + 构建前端，杀旧进程后启动
#   ./scripts/restart.sh --quick      # 不编译，只杀进程重启（需已有二进制与 dist）
#   ./scripts/restart.sh --no-web     # 编译 CLI，但不重新 npm build
#   ./scripts/restart.sh --release    # 用 release 二进制
#   ./scripts/restart.sh --bind 0.0.0.0:8765
#   ./scripts/restart.sh --daemon     # 后台运行，日志写入 .novelx/web.log
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BIND="${NOVELX_BIND:-127.0.0.1:8765}"
PORT="${BIND##*:}"
BUILD_CLI=1
BUILD_WEB=1
RELEASE=0
DAEMON=0
QUICK=0

usage() {
  cat <<'EOF'
一键重启 NovelX（Rust web 服务，默认托管 web/dist）

用法：
  ./scripts/restart.sh              # 编译 CLI + 构建前端，杀旧进程后启动
  ./scripts/restart.sh --quick      # 不编译，只杀进程重启（需已有二进制与 dist）
  ./scripts/restart.sh --no-web     # 编译 CLI，但不重新 npm build
  ./scripts/restart.sh --release    # 用 release 二进制
  ./scripts/restart.sh --bind 0.0.0.0:8765
  ./scripts/restart.sh --daemon     # 后台运行，日志写入 .novelx/web.log
EOF
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage ;;
    --quick) QUICK=1; BUILD_CLI=0; BUILD_WEB=0; shift ;;
    --no-web) BUILD_WEB=0; shift ;;
    --no-build) BUILD_CLI=0; BUILD_WEB=0; shift ;;
    --release) RELEASE=1; shift ;;
    --daemon|-d) DAEMON=1; shift ;;
    --bind) BIND="$2"; PORT="${BIND##*:}"; shift 2 ;;
    --bind=*) BIND="${1#*=}"; PORT="${BIND##*:}"; shift ;;
    *)
      echo "未知参数: $1（用 --help 查看用法）" >&2
      exit 2
      ;;
  esac
done

if [[ "$QUICK" -eq 1 ]]; then
  BUILD_CLI=0
  BUILD_WEB=0
fi

BIN_DIR="$ROOT/target/debug"
PROFILE_FLAG=""
if [[ "$RELEASE" -eq 1 ]]; then
  BIN_DIR="$ROOT/target/release"
  PROFILE_FLAG="--release"
fi
NOVEL_BIN="$BIN_DIR/novel"

kill_port() {
  local pids
  pids="$(lsof -tiTCP:"$PORT" -sTCP:LISTEN 2>/dev/null || true)"
  if [[ -n "${pids}" ]]; then
    echo "→ 停止占用 :$PORT 的进程: $pids"
    # shellcheck disable=SC2086
    kill $pids 2>/dev/null || true
    sleep 0.4
    pids="$(lsof -tiTCP:"$PORT" -sTCP:LISTEN 2>/dev/null || true)"
    if [[ -n "${pids}" ]]; then
      # shellcheck disable=SC2086
      kill -9 $pids 2>/dev/null || true
      sleep 0.2
    fi
  else
    echo "→ :$PORT 无监听进程"
  fi
}

echo "== NovelX 重启 =="
echo "  root: $ROOT"
echo "  bind: $BIND"

kill_port

if [[ "$BUILD_CLI" -eq 1 ]]; then
  echo "→ cargo build -p novelx-cli ${PROFILE_FLAG}"
  # shellcheck disable=SC2086
  cargo build -p novelx-cli ${PROFILE_FLAG}
else
  echo "→ 跳过 CLI 编译"
fi

if [[ ! -x "$NOVEL_BIN" ]]; then
  echo "错误: 找不到可执行文件 $NOVEL_BIN" >&2
  echo "请先运行: cargo build -p novelx-cli ${PROFILE_FLAG}" >&2
  exit 1
fi

if [[ "$BUILD_WEB" -eq 1 ]]; then
  echo "→ npm run build (web/)"
  if [[ ! -d "$ROOT/web/node_modules" ]]; then
    (cd "$ROOT/web" && npm install)
  fi
  (cd "$ROOT/web" && npm run build)
else
  echo "→ 跳过前端构建"
fi

mkdir -p "$ROOT/.novelx"
LOG="$ROOT/.novelx/web.log"
PID_FILE="$ROOT/.novelx/web.pid"

echo "→ 启动: $NOVEL_BIN web --bind $BIND"
if [[ "$DAEMON" -eq 1 ]]; then
  nohup "$NOVEL_BIN" web --bind "$BIND" >"$LOG" 2>&1 &
  echo $! >"$PID_FILE"
  sleep 0.5
  if ! kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
    echo "启动失败，见日志: $LOG" >&2
    tail -n 40 "$LOG" >&2 || true
    exit 1
  fi
  echo "✓ 已后台运行 pid=$(cat "$PID_FILE")"
  echo "  日志: $LOG"
  echo "  打开: http://${BIND}/"
else
  echo "✓ 前台运行（Ctrl+C 停止）→ http://${BIND}/"
  exec "$NOVEL_BIN" web --bind "$BIND"
fi
