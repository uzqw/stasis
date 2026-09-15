#!/usr/bin/env bash
# 跨层集成测试：rest-break(Rust) → immutable request events → Stasis session controller。
# 使用隔离 aide/AW/文件目录，不碰生产服务、不锁真屏。
# 行为规格：aide/rest-break/integration_test.sh（只读参考，不改）。
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "$0")/.." && pwd)"
AIDE_BIN="${AIDE_BIN:-$HOME/.local/bin/aide}"
REST_BREAK_BIN="${REST_BREAK_BIN:-$REPO_ROOT/rest-break/target/debug/rest-break}"
DRIVE_BIN="${DRIVE_BIN:-$REPO_ROOT/target/debug/examples/rest_session_drive}"
AIDE_PORT=18081
AW_PORT=18999
API_KEY=itest-key

for bin in "$AIDE_BIN" "$REST_BREAK_BIN" "$DRIVE_BIN"; do
  [[ -x "$bin" ]] || { echo "missing binary: $bin" >&2; exit 1; }
done

TMP="$(mktemp -d /tmp/rb-itest-XXXX)"
cleanup() {
  [ -n "${AIDE_PID:-}" ] && kill "$AIDE_PID" 2>/dev/null || true
  [ -n "${AW_PID:-}" ] && kill "$AW_PID" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

echo "== 1. mock ActivityWatch（180 分钟连续 not-afk → 疲劳 180 → due）=="
cat > "$TMP/aw_mock.py" <<'PYEOF'
import json, http.server, time
now = time.time()
START = now - 180*60
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.endswith('/buckets/'):
            body = json.dumps(["aw-watcher-afk_ITEST"]).encode()
        elif '/events' in self.path:
            ev = [{"timestamp": time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime(START)),
                   "duration": 180*60, "data": {"status": "not-afk"}}]
            body = json.dumps(ev).encode()
        else:
            self.send_response(404); self.end_headers(); return
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a): pass
http.server.HTTPServer(('127.0.0.1', 18999), H).serve_forever()
PYEOF
python3 "$TMP/aw_mock.py" & AW_PID=$!
sleep 0.5

echo "== 2. 隔离 HOME + 隔离 locker 事件目录 =="
mkdir -p "$TMP/home/.config/mcp" "$TMP/locker"
cat > "$TMP/home/.config/mcp/mcp.json" <<EOF
{"mcpServers": {"aide": {"url": "http://127.0.0.1:$AIDE_PORT/mcp",
  "headers": {"X-API-Key": "$API_KEY"}}}}
EOF
cat > "$TMP/locker/config.json" <<EOF
{"schedule_file": "$TMP/locker/input-locker-plan.json", "password": "123456"}
EOF

echo "== 3. 起隔离 aide 实例 =="
LOCKER_CONFIG="$TMP/locker/config.json" \
REPORT_DIR="$TMP" \
API_KEY="$API_KEY" \
PORT=$AIDE_PORT \
GITEA_BASE_URL=http://127.0.0.1:1 GITEA_TOKEN=x \
AW_BASE_URL="http://127.0.0.1:$AW_PORT/api/0" \
  "$AIDE_BIN" & AIDE_PID=$!
sleep 1

echo "== 4. 跑 rest-break Rust 二进制 =="
mkdir -p "$TMP/rbhome"
cp "$REST_BREAK_BIN" "$TMP/rbhome/rest-break"
HOME="$TMP/home" \
AW_BASE_URL="http://127.0.0.1:$AW_PORT/api/0" \
STATUS_FILE="$TMP/status.json" \
TZ=Asia/Shanghai \
  "$TMP/rbhome/rest-break" > "$TMP/rb.out" 2> "$TMP/rb.err" \
  || { echo "rest-break exit $? stderr:"; cat "$TMP/rb.err"; exit 1; }
cat "$TMP/rb.out"
grep -q '"due":true' "$TMP/rb.out" || { echo "FAIL: rest-break 未判定 due"; exit 1; }

echo "== 5. 断言：aide 写入不可变 rest.requested 事件 =="
EVENTS="$TMP/locker/input-locker-events"
python3 - "$EVENTS" <<'PYEOF'
import json, sys
from pathlib import Path
root = Path(sys.argv[1]) / 'requests'
files = sorted(root.glob('*.json'))
assert len(files) == 1, f"request files: {files}"
e = json.loads(files[0].read_text())
assert e['schema'] == 'input-locker.event/v1'
assert e['type'] == 'rest.requested'
assert e['sessionId'] and e['data']['lockAt'] < e['data']['unlockAt']
print(f"OK: request={files[0].name} session={e['sessionId']}")
PYEOF
[ ! -e "$TMP/locker/input-locker-plan.json" ] || { echo "FAIL: legacy plan was created"; exit 1; }

echo "== 6. 断言：Stasis session controller 允许立即自行解锁 =="
"$DRIVE_BIN" "$EVENTS"

echo "== 7. 断言：结果事件可被 aide 投影读取 =="
python3 - "$AIDE_PORT" "$API_KEY" <<'PYEOF'
import json, sys, urllib.request
port, key = sys.argv[1], sys.argv[2]
req = urllib.request.Request(f'http://127.0.0.1:{port}/mcp', method='POST',
    data=json.dumps({'jsonrpc':'2.0','id':1,'method':'tools/call',
        'params':{'name':'get_rest_sessions','arguments':{}}}).encode(),
    headers={'Content-Type':'application/json',
             'Accept':'application/json, text/event-stream',
             'X-API-Key':key})
raw = urllib.request.urlopen(req).read().decode()
data = next(line[6:] for line in raw.splitlines() if line.startswith('data: '))
outer = json.loads(data)
text = outer['result']['content'][0]['text']
sessions = json.loads(text)
assert sessions[0]['phase'] == 'ended', sessions
assert sessions[0]['segments'][0]['endReason'] == 'password', sessions
print('OK: aide projection exposes ended/password session')
PYEOF

echo
echo "=== 全部通过：rest-break → request event → Stasis session → aide projection ==="
