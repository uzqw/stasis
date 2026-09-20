# Stasis Windows — real-machine lock/unlock test record (leg 1)

Date: 2026-09-20. Machine: physical ASUS host, Windows 11 (build 10.0.26200),
user `admin`, console session 1 (no RDP session), process **unelevated**.
Build under test: `stasis.exe` sha256 `ed5882be…8ddd07` (commit `02243e4`,
`x86_64-pc-windows-gnu` release build, 8,505,914 bytes).

Performed under the authorized-real-desktop clause: the operator sat at the
machine and consented to every lock. Recovery channels were armed before the
first lock and one of them was used twice:

- command file `{"cmd":"unlock"}` in the events-dir parent → `ok` ack;
- `taskkill /IM stasis.exe /F` over SSH (session 0, unaffected by the
  session-1 hooks).

Event dir isolated to `<desktop>\stasis\events-test`; unlock password was a
test-only value in `%APPDATA%\stasis\config.json` (not a user secret).

## Test rig

- Access: workstation → `recolx` (WSL2) → `admin@192.168.3.194`. SSH lands in
  **session 0**, where neither the GUI nor its input is visible, so launching
  and probing went through scheduled tasks with an interactive token
  (session 1).
- Launch: task `stasis-test` → `.cmd` launcher setting
  `INPUT_LOCKER_EVENTS_DIR` and `RUST_LOG=info`. A plain
  `cmd /c set A && exe` did **not** propagate the variables; the `.cmd`
  launcher does.
- Remote input: `SendKeys` / `mouse_event` from a session-1 task. Low-level
  hooks see injected input; `LLKHF_INJECTED` is deliberately ignored by this
  backend, so injected and physical input take the same path (verified: the
  raw diagnostic log showed `flags=16` for injected keys, and the gesture
  state machine reacted identically).
- Observation: screen capture from session 1, the app log, and a *mouse
  swallow probe* — inject `mouse_event(MOVE)` and read `Cursor.Position`.
  0 px moved = hook installed and swallowing; movement = hook released. This
  removes the need to ask the operator whether the mouse "still works".
- Third, independent channel: **ActivityWatch on the target** (`aw-server`,
  `aw-watcher-afk`, `aw-watcher-window`, already running). Query
  `http://localhost:5600/api/0/buckets/<bucket>/events?start=…&end=…`, and
  read the `Became AFK` / `No longer AFK` lines in
  `Logs\aw-watcher-afk\*.log` (180 s timeout). It answers "was anyone at
  this machine at time T" without asking the operator, and it keeps
  answering while the lock swallows input.

## Per-case results

| Case | Method | Result |
| --- | --- | --- |
| Lock via UI button | operator clicked 锁定系统 | PASS — UI 已锁定, pointer frozen |
| Lock via command file | `{"cmd":"lock"}` + ack | PASS — `ok`, identical locked state |
| Keys reach the engine | 3× injected `j` while locked | PASS — log `unlock mode armed by gesture` |
| Gesture with physical keys | operator pressed `j`×3 on the machine keyboard | PASS — same log line, 12:56:00 |
| Mouse swallowed | injected move while locked | PASS — cursor moved 0 px |
| Password + Enter | injected password + `{ENTER}` | PASS — UI 已成功解锁 / 未锁定 |
| Hooks released on unlock | injected move after unlock | PASS — cursor moved 376 px |
| Unlock via command file | `{"cmd":"unlock"}` + ack | PASS — `ok`, lock released |
| Callback latency | locked ≈ 25 min at `RUST_LOG=info` | PASS — no `Health` line |
| Silent hook removal | watchdog / `Released` path | NOT EXERCISED — never triggered |
| UIPI boundary | keys aimed at an elevated window | NOT VERIFIED — no elevated user process existed on this machine |
| SAS escape | Ctrl+Alt+Del | NOT VERIFIED — cannot be injected |
| Full manual round | operator: lock → `j`×3 → password → Enter | PENDING — the password step was injected, not typed by hand |

Screenshots: [locked + armed](stasis-win-leg1-locked.png),
[unlocked](stasis-win-leg1-unlocked.png). No `rest.*` event is emitted for a
manual lock/unlock — correct, there is no active rest session; the isolated
events dir stayed empty.

## Findings

1. **「强制解锁（UI）」 is unusable while locked.** The mouse hook swallows
   clicks on the app's own window, so the one in-app escape hatch cannot be
   pressed exactly when it is needed. While locked the keyboard is the only
   in-app path; out-of-band recovery is the command file or killing the
   process. Not addressed in this leg (no code change).
2. **The operator's "3× `j` does nothing" report was two state errors, not a
   gesture defect.** (a) The first launch had no password configured, so
   锁定 was refused with 请先设置密码 and never installed a hook; (b) after the
   restart the operator confirmed they did not click 锁定系统 again, so the
   machine was never locked and keys never entered the app. With the lock
   engaged, physical `j`×3 armed unlock mode on the first attempt.
3. **The reported "later burst of operator key presses" was never typed on
   this machine.** ActivityWatch gives a minute-resolution input timeline:
   after the last input at ~12:57:23 (this leg's own injected probe) the
   idle timer was not refreshed until 13:00:38 — **3 min 15 s with no input
   of any kind**, exactly the stretch in which the operator reported that
   keys did nothing. The injected probe refreshed the timer while both hooks
   were swallowing, so the timer is not blind to hook-swallowed input. The
   window watcher agrees: `stasis.exe` held the foreground at every operator
   chat message in that window, so those messages came from another device,
   and the operator's one real touch (13:00:38) was swallowed as designed.
   The physical path itself is proven by the 12:56:00 gesture, and the
   machine has exactly one USB keyboard and one USB mouse, so no KVM or
   vendor input router is involved. Method note: the earlier evidence
   (password-dot count in a screenshot) could not separate "never delivered"
   from "delivered but unmapped", and this backend logs no per-key data by
   design — that is what made the episode slow to diagnose.
4. **No log line covers the lock path itself.** `UiCmd::Lock`,
   `start_lock()` outcomes and the command-file lock all change only the UI
   snapshot, so "was it ever locked?" cannot be answered from the log alone.
   The command-file ack and the UI capture are the only evidence.

## Notes / limits

- The password step was driven by injected input (the plaintext must not be
  written to any file, so it was read from the config by the injector); the
  physical path is covered by the physical `j`×3 gesture. A full round with a
  human hand on the password is still pending.
- Both hooks live for the whole lock; the watchdog's stall detection and the
  `Grab` join timeout were never exercised on this machine.
- Session 0 / session 1 separation is a property of this rig, not of the app.
