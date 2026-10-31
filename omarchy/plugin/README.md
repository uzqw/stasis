# Stasis bar widget for Omarchy

A `bar-widget` plugin for the Omarchy shell. It starts a timed, input-blocking
rest and shows the remaining time in the bar.

## Requirements

- Omarchy (Quickshell shell) with the plugin system.
- The `stasis` binary built from this repository, reachable at the path in the
  widget's `binary` setting (default `~/.local/bin/stasis`).
- The user in the `input` group, otherwise locking fails with
  `no input devices found`.

## Install

From the repository root:

```sh
./omarchy/install.sh
```

Or by hand:

```sh
cp -r omarchy/plugin ~/.config/omarchy/plugins/stasis.rest
omarchy-shell shell rescanPlugins
omarchy plugin enable stasis.rest --section right
```

The daemon (`stasis daemon`) must be running; `install.sh` installs and enables
the `stasis.service` user unit for it.

## Use

- Left click: open the duration panel (5/10/20/30 minutes by default).
- While resting the bar shows a countdown. The keyboard and pointer are grabbed
  at the kernel level, so the bar cannot be clicked until the timer ends.
- End a rest early: press `j` three times, then type your password and Enter.
- Right click: nothing (no accidental locks).

The panel shows a warning colour when the last attempt failed; run
`stasis status` for the engine's message.

When `rest-break` is deployed, the bar also shows the current work stretch
(e.g. `1h12m`) while idle and the panel adds `WORKING` / `NEXT REST` rows.
That data comes from rest-break's `status.json` (ActivityWatch-backed) and is
purely informational: it does not affect starting or ending a rest.

## Settings

| Key | Default | Meaning |
| --- | --- | --- |
| `binary` | `~/.local/bin/stasis` | stasis executable |
| `durations` | `5,10,20,30` | comma-separated minutes |
| `statusFile` | `~/.local/state/stasis/status.json` | engine snapshot |
| `restBreakStatusFile` | auto-detect | optional rest-break `status.json` |

## IPC

```sh
omarchy-shell stasis.rest open
omarchy-shell stasis.rest rest 20
```

See `docs/omarchy.md` in the repository for design notes and limitations.
