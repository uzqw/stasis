#!/usr/bin/env bash
# Install the Stasis daemon + Omarchy bar widget for the current user.
#
# Safe to re-run.  Nothing here grabs input, changes groups, or needs root:
# the daemon starts idle and only grabs devices when a rest is requested.
set -euo pipefail

with_afk=0
for arg in "$@"; do
  case "$arg" in
    --with-afk) with_afk=1 ;;
    *) echo "error: unknown option: $arg" >&2; exit 2 ;;
  esac
done

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
plugin_id="stasis.rest"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
plugin_dir="$config_home/omarchy/plugins/$plugin_id"
unit_dir="$config_home/systemd/user"
bin_dir="$HOME/.local/bin"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found; install Rust first (https://rustup.rs)" >&2
  exit 1
fi

echo "==> building stasis"
(cd "$repo_dir" && cargo build --release --bin stasis)

echo "==> installing binary to $bin_dir/stasis"
install -Dm755 "$repo_dir/target/release/stasis" "$bin_dir/stasis"

echo "==> installing plugin into $plugin_dir"
mkdir -p "$(dirname "$plugin_dir")"
if [ -L "$plugin_dir" ]; then
  rm "$plugin_dir"
elif [ -e "$plugin_dir" ]; then
  mv "$plugin_dir" "$plugin_dir.bak-$(date +%s)"
fi
cp -r "$repo_dir/omarchy/plugin" "$plugin_dir"

echo "==> installing systemd user unit"
install -Dm644 "$repo_dir/omarchy/stasis.service" "$unit_dir/stasis.service"
systemctl --user daemon-reload
systemctl --user enable stasis.service
# restart, not `enable --now`: a running daemon would keep the old binary.
systemctl --user restart stasis.service

if [ "$with_afk" = 1 ]; then
  echo "==> building rest-break"
  (cd "$repo_dir" && cargo build --release --manifest-path rest-break/Cargo.toml)
  install -Dm755 "$repo_dir/rest-break/target/release/rest-break" "$bin_dir/rest-break"

  echo "==> installing AFK publisher + status timer"
  install -Dm644 "$repo_dir/omarchy/stasis-afk.service" "$unit_dir/stasis-afk.service"
  # Recovery multipliers live in a config file, not in the unit.
  install -d "$HOME/.config/rest-break"
  if [ ! -f "$HOME/.config/rest-break/config.json" ]; then
    printf '{\n  "afkRecovery": 0.2,\n  "lockedRecovery": 6.0\n}\n' \
      > "$HOME/.config/rest-break/config.json"
  fi
  install -Dm644 "$repo_dir/omarchy/rest-break-check.service" "$unit_dir/rest-break-check.service"
  install -Dm644 "$repo_dir/omarchy/rest-break-check.timer" "$unit_dir/rest-break-check.timer"
  install -Dm644 "$repo_dir/omarchy/stasis-autorest.service" "$unit_dir/stasis-autorest.service"
  install -Dm644 "$repo_dir/omarchy/stasis-autorest.timer" "$unit_dir/stasis-autorest.timer"
  systemctl --user daemon-reload
  systemctl --user enable stasis-afk.service
  systemctl --user restart stasis-afk.service
  systemctl --user enable --now rest-break-check.timer
  systemctl --user enable --now stasis-autorest.timer
  systemctl --user start rest-break-check.service || true
fi

echo "==> registering the bar widget"
omarchy-shell shell rescanPlugins
omarchy plugin enable "$plugin_id" --section right

cat <<'EOF'

Installed.

  stasis status                    # daemon snapshot
  omarchy plugin list | grep stasis
  systemctl --user status stasis

Locking needs read access to /dev/input/event*.  If `stasis status` reports
"no input devices found", add yourself to the input group and log back in:

  sudo usermod -aG input $USER

Work time in the bar (optional, --with-afk) needs an ActivityWatch server on
http://localhost:5600 (aw-server or aw-server-rust) and the input group, so
`stasis afk` can read evdev.  See docs/omarchy.md.
EOF
