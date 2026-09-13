# Stasis Linux — real-desktop lock/unlock test record (leg 2)

Date: 2026-09-13. Machine: KDE Plasma + Wayland, user in `input` group,
`/dev/uinput` writable. Performed under the authorized-real-desktop clause.
Recovery channels armed throughout: a uinput virtual keyboard able to inject
the unlock sequence while real devices were grabbed, `kill <pid>`, and the
UI 强制解锁 button.

Test rig: `STASIS_DATA_DIR`/`STASIS_CONFIG`/`STASIS_EVENTS_DIR` pointed at an
isolated dir under `/tmp/stasis-leg2` (single-consumer rule respected — not
the legacy `../input-locker` dir). Password under test: `123456`.

## Defect found and fixed

- **Engine `crossbeam_channel::Select` panic.** The engine thread panicked
  ~1 s after launch ("dropped `SelectedOperation` without completing the
  operation") because the `tick` arm never called `oper.recv(&tick_rx)`.
  Effect: the whole engine loop died on every launch — command file never
  polled, lock/unlock dead. This is the leg-1 "caveat" confirmed as a real
  single-launch defect, not a double-instance artifact.
  Fix: complete the selected op (`let _ = oper.recv(&tick_rx);`) in
  `src/engine.rs`. After the fix the engine survives the tick, polls the
  command file, and locks/unlocks correctly.

## Per-case results

| Case | Method | Result |
| --- | --- | --- |
| Lock via UI button | uinput absolute pointer clicked 锁定系统 | PASS — UI → 已锁定 |
| Real input swallowed | `evtest` on the injected device while locked | PASS — "device is grabbed by another process", no events leak to session |
| Unlock gesture | uinput kb: CapsLock×3 → `123456` → Enter | PASS — armed, dots shown, unlocked to 未锁定 |
| Caps outside 2 s window | caps, +2.5 s, caps, +2.5 s, caps | PASS — did not arm |
| Auto-repeat | 8× `value=2` caps events | PASS — did not arm (backend filters `value==1`) |
| Wrong password + retry | `999999` → error shown → `123456` | PASS — 密码错误 shown, retry unlocked |
| lock→unlock→lock | command-file lock ×3 | PASS — re-grabs cleanly each time |
| Clean exit while locked | `kill` while locked, then probe | PASS — grab released, evtest sees keys again |

Legend: PASS = observed correct on the real desktop.

## Notes / limits

- No `rest.unlocked` event is emitted on manual lock/unlock — correct: there
  is no active rest session; `controller.unlock` returns `no_rest_session`
  and the UI shows 已成功解锁.
- Wayland has no global-input protocol; the grab is at the evdev layer, so a
  nested KWin window does not confine it. Nested KWin was used to verify the
  UI renders/launches correctly in an isolated session (screenshot below);
  real-grab cases ran on the authorized desktop.
- Command-file `lock`/`unlock` polled and acked correctly
  (`input-locker-command.json` → `input-locker-ack.json`).

## Evidence (committed under `docs/notes/`)

- `stasis-leg2-locked.png` — UI 已锁定 after UI-button lock.
- `stasis-leg2-unlockmode.png` — armed, 输入密码解锁 + dots.
- `stasis-leg2-wrongpwd.png` — 密码错误 after wrong password.
- `stasis-leg2-unlocked.png` — back to 未锁定 after correct gesture.
- `stasis-leg2-nested.png` — Stasis rendering inside nested KWin
  (`wayland-stasis`, 1280×800), CJK clean, no tofu.
