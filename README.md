# Stasis

> **Pause the machine. Rest the human.**

A Rust rewrite of Input Locker: freezes keyboard and mouse input so the human can
rest. Three desktop targets — Windows, Linux (X11/Wayland via evdev), macOS.

[中文文档](./README.zh-CN.md)

Core principle: **one behavior spec, platforms only adapt input capture and OS resources;
unlocking never depends on window focus or UI responsiveness.**

## Status

| Platform | State |
|---|---|
| Linux | Verified end-to-end on a real KDE Plasma + Wayland desktop (evdev grab, `j`×3 gesture, password unlock, command-file protocol). |
| Windows | Verified on a physical Windows 11 machine (console session, unelevated). Lock/unlock main path confirmed; some edge cases open (see below). |
| macOS | Implemented (CGEventTap backend). **Compiles and launches; real-desktop lock/unlock not yet verified.** Requires Accessibility permission. |

Unlock gesture (since 2026-09-14): **press `j` three times within 2 seconds**,
then type the password and Enter. Older notes referencing CapsLock×3 are stale.

- [Design](docs/design.md) — scope, cross-platform behavior, thread/resource
  model, protocol compatibility, security boundaries.
- [Implementation & acceptance plan](docs/implementation-plan.md).
- [ADR 0001](docs/adr/0001-rust-rewrite-shared-core.md) — why a rewrite, why a
  shared core.
- Verification notes live under [docs/notes/](docs/notes/) (Linux leg2,
  Windows leg1, Windows unlock incident 2026-09-23).

## Layout

```text
src/            stasis GUI + engine + platform backends (linux/windows/macos)
rest-break/     standalone crate: "when to rest" decision core + status UI
examples/       manual real-machine test drivers
tools/guard/    repo guard (docs, line width, secrets), zero-dep Rust bin
```

## Build

Requires Rust 1.85+ (edition 2024).

```sh
cargo build --release --bin stasis
cargo build --release --features ui --bins --manifest-path rest-break/Cargo.toml
```

Produces `target/release/stasis`, `rest-break/target/release/rest-break`, and
`rest-break/target/release/rest-break-ui`.

### macOS notes

macOS builds need no extra system deps beyond Xcode CLT — `core-foundation` /
`core-graphics` are cargo deps. `rest-break-ui` picks a CJK font from
`/System/Library/Fonts` (PingFang/STHeiti/Hiragino). The event tap needs
**Accessibility permission** (System Settings → Privacy & Security →
Accessibility); without it `CGEventTapCreate` returns NULL and the app tells you
which pane to open.

Deploying stasis + rest-break + their launchd agents on macOS looks like:

```sh
# on the mac
cargo build --release --bin stasis
(cd rest-break && cargo build --release --features ui --bins)

# ~/Library/LaunchAgents/*.plist, see examples below
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.stasis.stasis.plist
```

`rest-break` further needs an [aide](https://github.com/…) MCP endpoint
(`~/.config/mcp/mcp.json`) and an ActivityWatch server with an
`aw-watcher-afk_*` bucket. aw-server-rust alone is not enough — the AFK watcher
is a separate client (the official ActivityWatch.app ships one; only that binary
is needed, no aw-qt). Cross-compile the Go aide binary with
`CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 go build`.

A minimal plist set (paths assume `~/stasis` checkout):

- `com.stasis.stasis.plist` — RunAtLoad + KeepAlive, program
  `~/stasis/target/release/stasis`
- `com.stasis.rest-break.plist` — StartInterval 60, program
  `~/stasis/rest-break/target/release/rest-break`, env
  `STATUS_FILE=~/stasis/rest-break/target/release/status.json`
- `com.stasis.rest-break-ui.plist` — RunAtLoad + KeepAlive, same `STATUS_FILE`
- `com.stasis.aw-server.plist`, `com.stasis.aw-watcher-afk.plist`,
  `com.stasis.aide.plist` — optional, if running the full stack on that machine

### Linux

```sh
cargo build --release
./target/release/stasis
```

Needs `input` group access to grab `/dev/input/event*`. Real grabbing is
high-risk: only run on an authorized desktop with a recovery channel ready
(uinput injection, `kill`, UI force-unlock button).

App log: `~/.config/stasis/stasis.log.YYYY-MM-DD` (UTC-rotated, **not**
journald). `RUST_LOG=info` for more detail.

On the dev machine (Manjaro) two systemd user services run the repo artifacts
directly: `stasis.service` → `target/release/stasis`, `rest-break-ui.service` →
`rest-break/target/release/rest-break-ui` (both on `graphical-session.target`).
Deploy = rebuild both crates from the target commit **and** restart both
services; building alone is not deploying.

### Windows

Cross-compile from Linux: `cargo build --release --target
x86_64-pc-windows-gnu`. Verified on real hardware; see the notes for what is and
isn't covered (UIPI boundary, Ctrl+Alt+Del exit, watchdog paths are untested on
real machines).

## rest-break

`rest-break/` is a standalone crate owning the "when to rest" decision. It talks
to ActivityWatch (:5600) and the aide MCP, writes `status.json`, and submits
`rest.requested` events when due. `rest-break-ui` is a small always-on-top
status window reading that file every second (hover to expand, click to pin).
GUI deps are behind the `ui` feature so the cron binary stays lean.

```sh
cargo test --manifest-path rest-break/Cargo.toml
cargo run --manifest-path rest-break/Cargo.toml --features ui --bin rest-break-ui
```

Env vars: `STATUS_FILE`, `RB_UI_CORNER` (`ne|nw|se|sw`, default `se`),
`RB_UI_MARGIN` (default `90`), `RB_UI_FONT_SIZE` (default `14`),
`RB_UI_TEXT_COLOR` (`#rrggbb`).

The root `cargo test`/`clippy` do not cover this crate; run the commands above
with `--manifest-path rest-break/Cargo.toml`.

## Conventions

- Commit messages, branches, AI disclosure: [CONTRIBUTING.md](CONTRIBUTING.md)
  (commits are in English).
- Agent contract, docs rules, forbidden zones: [AGENTS.md](AGENTS.md).
- Doc index: [docs/README.md](docs/README.md).

After cloning or creating a worktree, install the hooks once (`core.hooksPath`
is local config and does not propagate):

```sh
git config core.hooksPath .githooks
```
