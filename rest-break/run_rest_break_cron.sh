#!/usr/bin/env bash
# 每分钟轻量检查；只有本地判定 due 才提交 rest.requested 锁屏请求。
# Go 版行为规格：aide/rest-break/run_rest_break_cron.sh（只读参考，不改）。
set -euo pipefail
SKILL_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
LOG_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/rest-break"
BIN="${REST_BREAK_BIN:-$SKILL_DIR/target/debug/rest-break}"
# WSL 下 AW 跑在 Windows 主机，网关 IP 每台机器不同，从默认路由自动发现；
# 非 WSL 环境不设置，二进制默认 localhost:5600。可用 AW_BASE_URL 覆盖。
if [[ -z "${AW_BASE_URL:-}" ]] && grep -qi microsoft /proc/version 2>/dev/null; then
  gw="$(ip route 2>/dev/null | awk '/^default / {print $3; exit}')"
  export AW_BASE_URL="http://${gw:-localhost}:5600/api/0"
fi
# 状态文件给小窗 UI 轮询；WSL 下写到 Windows 可读路径（Windows 用户名按需调整）。
if [[ -z "${STATUS_FILE:-}" ]] && grep -qi microsoft /proc/version 2>/dev/null; then
  export STATUS_FILE="/mnt/c/Users/admin/Downloads/rest-break-status.json"
fi
export TZ=Asia/Shanghai
mkdir -p "$LOG_DIR"
exec 9>"$SKILL_DIR/.run.lock"
flock -n 9 || exit 0
cd "$SKILL_DIR"
if [[ "${1:-}" == "--check" ]]; then
  exec "$BIN" --check
fi
if [[ $# -ne 0 ]]; then
  echo "Usage: $0 [--check]" >&2
  exit 2
fi
LOG="$LOG_DIR/cron-$(date +%Y%m%d).log"
find "$LOG_DIR" -name 'cron-*.log' -mtime +6 -delete 2>/dev/null || true
exec >> "$LOG" 2>&1
# 每分钟轮询：只在 due 或出错时记日志；非 due 判定可用 --check 手动看。
if OUT="$(timeout --kill-after=10s 300s "$BIN" 2>&1)"; then
  if grep -qE '"due":true|"reason":"error:' <<<"$OUT"; then
    echo "$(date '+%F %T') $OUT"
  fi
else
  status=$?
  echo "$(date '+%F %T') 检查失败或超时（exit=$status），保留不确定执行保护"
  echo "$OUT"
  exit "$status"
fi
# 生成 overlay 显示文本（status.json 已更新，提取一行给桌面 overlay）。
STATUS_JSON="${STATUS_FILE:-$SKILL_DIR/status.json}"
if [[ -f "$STATUS_JSON" ]]; then
  python3 - "$STATUS_JSON" > /tmp/rest-break-overlay.txt 2>/dev/null <<'PYEOF' || true
import json, sys

d = json.load(open(sys.argv[1]))
if d.get("state") == "error":
    print("疲劳: 数据不可用")
else:
    f = d.get("fatigueMinutes", 0)
    nxt = (d.get("nextRestAt") or "")[11:16]
    dur = d.get("nextRestMinutes", 0)
    print(f"疲劳 {f:.0f} 分钟 · 下次 {nxt} 开始 · {dur} 分钟")
PYEOF
fi
