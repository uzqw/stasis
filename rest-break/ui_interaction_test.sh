#!/usr/bin/env bash
# rest-break-ui 交互的隔离验证：在专用 Xvfb（真 X 服务器）上用 XTEST 注入真指针
# 事件，逐步核对窗口几何——悬停展开 9 字段详情、移出 0.6s 收起、点击钉住/取消。
# 不碰真实桌面（:1）与真实输入设备。依赖：Xvfb、xdotool；截图需要 ImageMagick
# （缺失则跳过截图，仍核对几何）。
# 用法：bash rest-break/ui_interaction_test.sh   截图留在 $OUT（缺省 /tmp/rb-ui-interaction）
set -u

DISP=${DISP:-:98}
OUT=${OUT:-/tmp/rb-ui-interaction}
ROOT=$(cd "$(dirname "$0")" && pwd)
BIN=$ROOT/target/debug/rest-break-ui
COLLAPSED=46   # 收起态高度（egui points）：见 rest-break-ui.rs window_size()
EXPANDED=244   # 展开态高度：46 + 9 行 * (14 字号 + 8)

fail() { echo "FAIL: $*"; exit 1; }

if [ ! -x "$BIN" ]; then
  echo "== 构建 rest-break-ui =="
  cargo build --manifest-path "$ROOT/Cargo.toml" --features ui \
    --bin rest-break-ui || fail "构建失败"
fi

mkdir -p "$OUT"
cat > "$OUT/status.json" <<'JSON'
{
  "state": "ok",
  "fatigueMinutes": 55.5,
  "circadian": 1.4,
  "workMinutes": 39.8,
  "due": false,
  "restMinutes": 0,
  "nextRestAt": "2026-09-17T23:42:00+08:00",
  "nextRestMinutes": 10,
  "cooldownUntil": "",
  "reason": "below_fatigue_threshold:fatigue=55.5,circadian=1.4",
  "checkedAt": "2026-09-17T23:27:00+00:00"
}
JSON

Xvfb "$DISP" -screen 0 1920x1080x24 > "$OUT/xvfb.log" 2>&1 &
XVFB=$!
APP=0
cleanup() {
  [ "$APP" != 0 ] && kill "$APP" 2>/dev/null
  kill "$XVFB" 2>/dev/null
}
trap cleanup EXIT

for _ in $(seq 20); do DISPLAY=$DISP xdpyinfo >/dev/null 2>&1 && break; sleep 0.3; done
DISPLAY=$DISP xdpyinfo >/dev/null 2>&1 || fail "Xvfb $DISP 未就绪"

# 不继承真实会话的 WAYLAND_DISPLAY/XDG_SESSION_TYPE，否则会连到真桌面。
STATUS_FILE=$OUT/status.json RB_UI_CORNER=se RB_UI_MARGIN=90 DISPLAY=$DISP \
  env -u WAYLAND_DISPLAY -u XDG_SESSION_TYPE "$BIN" > "$OUT/app.log" 2>&1 &
APP=$!
sleep 4
export DISPLAY=$DISP

W=$(xdotool search --name rest-break-ui | head -1)
[ -n "$W" ] || fail "窗口未出现"

# 指针目标点取窗口中心：几何随展开态变化，不能硬编码坐标。
center() {
  eval "$(xdotool getwindowgeometry --shell "$W")"
  echo $((X + WIDTH/2)) $((Y + HEIGHT/2))
}
height() { eval "$(xdotool getwindowgeometry --shell "$W")"; echo "$HEIGHT"; }
shot() { command -v import >/dev/null && import -window "$W" "$OUT/$1.png"; }
check() { # check <期望> <说明>
  local h; h=$(height)
  [ "$h" = "$1" ] || fail "$2：期望高度 $1，实际 $h"
  echo "ok  $2（高度 $h）"
  shot "$3"
}

check "$COLLAPSED" "初始收起" 0-collapsed

read -r CX CY <<<"$(center)"; xdotool mousemove --sync "$CX" "$CY"; sleep 0.8
check "$EXPANDED" "悬停展开详情" 1-hover-in

xdotool mousemove --sync 5 5; sleep 0.3
check "$EXPANDED" "移出 0.3s 仍在宽限内" 2-grace
sleep 1.5
check "$COLLAPSED" "移出 1.8s 后收起" 3-collapsed-again

read -r CX CY <<<"$(center)"; xdotool mousemove --sync "$CX" "$CY"; sleep 0.5
xdotool click 1; sleep 0.5
xdotool mousemove --sync 5 5; sleep 1.8
check "$EXPANDED" "点击钉住后移出仍展开" 4-pinned-out

read -r CX CY <<<"$(center)"; xdotool mousemove --sync "$CX" "$CY"; sleep 0.5
xdotool click 1; sleep 0.5
xdotool mousemove --sync 5 5; sleep 1.8
check "$COLLAPSED" "再次点击取消钉住后收起" 5-unpinned

kill "$APP" 2>/dev/null; APP=0; sleep 0.5
pgrep -x rest-break-ui >/dev/null && fail "退出后仍有残留进程"
echo "ok  退出无残留进程"
echo "PASS: 交互三态与宽限收起全部符合预期；截图与日志：$OUT"
