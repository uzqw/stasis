# Stasis Windows — real-machine lock/unlock test record (leg 1)

Date: 2026-09-20. Machine: physical ASUS host, Windows 11 (build 10.0.26200),
user `admin`, console session 1 (no RDP session), process **unelevated**.
Build under test: `stasis.exe` sha256 `ed5882be…8ddd07` (commit `02243e4`,
`x86_64-pc-windows-gnu` release build, 8,505,914 bytes).

Performed under the authorized-real-desktop clause: the operator sat at the
machine and consented to every lock. Recovery channels were armed before the
first lock and the command file was used for recovery several times:

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
- Diagnostic per-key build: `stasis-diag.exe` is the same source with a
  temporary `tracing::debug!(msg, vk, scan, flags, "hook raw key")` added to
  `keyboard_proc` **before** the swallow decision, so the line is written for
  every event that reaches the hook, swallowed or not. Launched from its own
  `.cmd` with `RUST_LOG=info,stasis=debug` and its own task. `flags=0` means
  physical, `flags=16` is `LLKHF_INJECTED`; a real `j` carries `scan=36`, an
  injected one `scan=0`. Caution: **hooks exist only while locked**, so a
  diagnostic build that is not locked logs nothing — injecting at an unlocked
  instance proves nothing about the hook (that cost one false negative).
- Command-file gotcha: the record needs an `id`. A bare `{"cmd":"lock"}` is
  claimed by the app, fails to deserialize, and is deleted **silently** — no
  ack, no log line. Always write `{"cmd":"lock","id":"…"}`.
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
| Gesture with physical keys | operator pressed `j`×3 on the machine keyboard | PASS — 12:56:00, and again 14:21:32 / 14:34:56 |
| Mouse swallowed | injected move while locked | PASS — cursor moved 0 px |
| Password + Enter | injected password + `{ENTER}` | PASS — UI 已成功解锁 / 未锁定 |
| Hooks released on unlock | injected move after unlock | PASS — cursor moved 376 px |
| Unlock via command file | `{"cmd":"unlock"}` + ack | PASS — `ok`, lock released |
| Callback latency | locked ≈ 25 min at `RUST_LOG=info` | PASS — no `Health` line |
| Silent hook removal | watchdog / `Released` path | NOT EXERCISED — never triggered |
| UIPI boundary | keys aimed at an elevated window | NOT VERIFIED — no elevated user process existed on this machine |
| SAS escape | Ctrl+Alt+Del | NOT VERIFIED — cannot be injected |
| Full manual round | operator: 锁定系统 → `j`×3 → password → Enter | PASS — see below |

- Full manual round (14:34) — the operator locked with the UI button at
  14:34:16, their physical `j`×3 armed unlock mode at 14:34:56
  (`msg=256 vk=74 scan=36 flags=0`), they typed the password and Enter by
  hand, and the UI returned to 未锁定; the swallow oracle went 0 px while
  locked → 470 px after. No log line is written for a successful unlock, so
  the evidence is the gesture line, the oracle flip and the operator's own
  report (终于第一次成功解锁了).

Screenshots: [locked + armed](stasis-win-leg1-locked.png),
[unlocked](stasis-win-leg1-unlocked.png). No `rest.*` event is emitted for a
manual lock/unlock — correct, there is no active rest session; the isolated
events dir stayed empty.

## Fix and re-verification (same day, after the record above)

Root cause of the operator's report («`j`×3 arms unlock mode, then the password
does nothing; `Tab` then `Enter` is the only way out»): **Windows stops calling
the low-level keyboard hook while a window of the hooking process owns the
foreground.** With the lock window focused, every key is delivered straight to
that window: the engine never sees it, and egui — which is what `Tab`+`Enter`
reached — still activates 强制解锁（UI）. That is exactly the `unlocked (manual)`
pair at 14:48/14:50 in the log: injected input, not a mouse click.

Evidence (one instance, per-key diagnostic build, 15:22–15:23 local, lines from
`%APPDATA%\stasis\stasis.log.2026-09-20`):

- `j`×3 while another process held the foreground: every raw key logged with
  `age_ms=0`, then `unlock mode armed by gesture`.
- The lock window takes the foreground (winit's `force_window_active`: an
  injected `VK_LMENU` pair plus `SetForegroundWindow`, triggered by the arming
  code's own `ViewportCommand::Focus`). The next key produced **no** raw-key
  line, while the UI thread logged
  `ui saw key press while locked (hook leak)` for that same key.
- `UnhookWindowsHookEx` at the end still reported the hook as attached: Windows
  had not removed it, it simply was not called.
- Minimising our own window (same hook, nothing reinstalled): the next key was
  logged and reached the engine again.
- Restoring the window *without* giving it the foreground: still captured, as
  were the operator's physical backspaces in the same window.

So this is not a hook timeout, not the USB switch and not a lost event: it is
foreground ownership. Three small changes:

- `src/ui.rs` no longer sends `ViewportCommand::Focus` when the gesture arms
  (the `focus_request` field is gone). The window stays `AlwaysOnTop`, so the
  state is still visible, but it never takes the keyboard focus.
- `src/platform/windows.rs` `release_foreground()` runs right after the hooks
  are installed and hands the foreground to the next visible window in the
  z-order that is not ours. This covers the operator's own path — locking with
  the 锁定系统 button — where the window already holds the focus.
- Safety net: while locked, a key that reaches egui is proof capture stopped.
  `src/ui.rs` sends `UiCmd::Rehook`, the engine logs
  `input reached the window while locked; re-arming input capture`, rebuilds the
  grab (which re-runs the foreground handoff) and clears the partially typed
  password, so the next `Enter` cannot compare a buffer that is silently missing
  characters. If the re-arm fails, the lock is released instead of pretending.

Re-verification on the same machine (build `stasis.exe` sha256 `7aa064b0…44613a`,
installed over the leg-1 binary; per-key logging reverted before the build, so
the evidence is the production `info` log):

| Path | Result |
| --- | --- |
| 锁定系统 via UI with the window focused | PASS — `locked by ui`, foreground handed to another process, mouse swallowed (0 px) |
| that lock, then `j`×3 + `abc123`+Enter | PASS — `unlock mode armed by gesture`, `unlocked (password)`, no leak line, hooks released (617 px) |
| command-file lock, then `j`×3 + password | PASS — `unlocked (password)` |
| forced failure: our window pushed back to the foreground while locked | PASS — one leak line, `input capture re-armed`, foreground moved away, keys captured again |

Findings updated:

1. **was: 「强制解锁（UI）」 is unusable while locked.** Still unreachable, now
   for a second reason: with capture healthy the keyboard is swallowed again, so
   `Tab` is an unmapped key and `Enter` submits the password buffer. While the
   lock works the button is mouse-only and the mouse hook eats the click; the
   in-app path is the password, out-of-band recovery stays the command file and
   killing the process.
5. **follow-up.** A keyup without its keydown means the hook was not called for
   that event and the event went to the target window instead — the same
   observable family as the foreground leak above (both are «hook not called,
   key delivered»). The 14:21 window had two instances writing one log file, so
   that single event stays unattributed; it no longer needs a separate
   explanation.

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
3. **The reported "bursts of operator key presses" never reached this
   machine** — the hook chain is not the defect. Settled by a positive
   control plus two independent negative channels:
   - Positive control: with the per-key diagnostic build locked at 14:21:28,
     the operator's `j`×3 arrived as `msg=256 vk=74 scan=36 flags=0` —
     `flags=0` physical, `scan=36` a real scancode — and armed unlock mode;
     the same build logs injected keys as `scan=0 flags=16`. The hook
     therefore sees physical keys whenever they do arrive, and it logs them
     *before* deciding to swallow.
   - ActivityWatch input timeline: after the last input at ~12:57:23 (this
     leg's own injected probe) the idle timer was not refreshed until
     13:00:38 — **3 min 15 s with no input of any kind**, exactly the stretch
     in which the operator reported that keys did nothing. The injected
     probe refreshed the timer while both hooks were swallowing, so the
     timer is not blind to hook-swallowed input.
   - Session-1 sampler during the second episode: from 14:14:41 to 14:18:02
     the machine recorded non-stop mouse input ticks with **zero** key
     edges, and the diagnostic build (locked 14:11:50–14:14:22) wrote no
     key line at all. Mouse present, keyboard absent.

   Cause: the operator's **keyboard sits behind a USB switch** (their
   answer: only the keyboard is switched, the mouse stays on this machine),
   and in those windows it was routed to the device they were chatting
   from. The window watcher agrees: `stasis.exe` held the foreground at
   every operator chat message in the first window, so those messages came
   from another device. The earlier reading "one USB keyboard device is
   present, so no KVM is involved" was wrong — a switch presents itself to
   whichever host it is attached to as a single HID keyboard, so device
   count says nothing about the wiring. Method note: the earlier evidence
   (password-dot count in a screenshot) could not separate "never delivered"
   from "delivered but unmapped", and this backend logs no per-key data by
   design — that is what made the episode slow to diagnose.
4. **No log line covers the lock path itself.** `UiCmd::Lock`,
   `start_lock()` outcomes and the command-file lock all change only the UI
   snapshot, so "was it ever locked?" cannot be answered from the log alone.
   A failed `start_lock()` is worse: it sets a UI message only, so a lock
   that never took leaves **no** trace anywhere but the screen. The
   swallow oracle (`mouse_event(MOVE,150,90)` → 0 px locked, 250–520 px
   released) was what made the state measurable at all; the command-file ack
   and the UI capture are the only other evidence.
5. **One physical keydown had no hook line while its keyup did.** In the
   14:21:35 diagnostic window the operator's `a` appears only as
   `msg=257 vk=65 scan=30 flags=128` (keyup); its `msg=256` keydown is
   missing, while every other event of that window — physical and injected —
   is present and no `Health` line was written. A low-level callback is the
   first user-mode consumer of a key event, so a skipped event means the
   callback was not called for it; the documented silent-removal gap is about
   the hook as a whole, not a single event. Single occurrence, not
   reproducible here; recorded so a future drop can be matched against a
   diagnostic log.

## Notes / limits

- The manual round typed the password by hand; the injected run kept the
  plaintext out of every file by reading it from the config inside the
  injector. Successful unlocks are logged as `unlocked (password)` /
  `unlocked (manual)` / `unlocked (command)` since commit `18be1c9`, so the
  manual round rests on that line plus the operator's confirmation and the
  oracle flipping from 0 px to 470 px.
- The target desktop's `stasis.exe` is now the fixed build (`7aa064b0…44613a`,
  the commit that follows `18be1c9`); the leg-1 sha256 `ed5882be…8ddd07` above
  stays as the historical artifact this record was written against.
- Both hooks live for the whole lock; the watchdog's stall detection and the
  `Grab` join timeout were never exercised on this machine, and the new
  re-arm path is exercised only by forcing the foreground back onto our own
  window.
- Session 0 / session 1 separation is a property of this rig, not of the app.
