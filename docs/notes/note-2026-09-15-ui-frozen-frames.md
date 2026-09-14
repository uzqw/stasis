# Stasis UI 冻结：手势生效但界面不更新

日期：2026-09-14 23:55 – 2026-09-15 01:00（本地 UTC+8，即 UTC 2026-09-14 15:55–17:00）。
环境：授权真实桌面（KDE Plasma + Wayland），用户属 `input` 组。

## 现象

锁定时连按 3 次 `j`，界面毫无变化；窗口文字重影、闪烁，还能透出桌面背景；输完密码后界面恢复。

## 结论：手势链路正常，坏的是 UI 渲染与刷新

`~/.config/stasis/stasis.log.2026-09-14` 里 `unlock mode armed by gesture` 出现在 16:00:26、
16:12:54、16:13:13、16:16:02（UTC），全部来自用户真实键盘。16:12–16:16 期间没有任何 `rest.*`
会话事件，说明这几次是「UI 按钮锁定 → 用户按 `j`」，**手势每次都成功进入解锁模式**，密码解锁也
成功（16:14 用注入复核：`rest.locked` → `rest.unlocked(reason=password)`）。

用户之所以看到「按了没反应」，是下面三个缺陷叠加。

## 三个叠加的 UI 缺陷

1. **窗口没有背景。** eframe 0.36 的 `App::ui` 交给实现者的 `Ui` 既无 margin 也无 background
   （`epi.rs` 明确要求自行套 panel），而 `clear_color` 默认 alpha=180，于是桌面和上一帧的残留像素
   都透了出来。
2. **从不请求重绘。** 代码里没有任何 `request_repaint()`；锁定期间 Stasis 抓走全部输入，egui 收不到
   事件就不会重绘 —— 状态已经变了（日志为证），屏幕却停在旧帧。
3. **每帧都发 viewport 命令。** `logic()` 在未锁定时每帧发一次 `WindowLevel`，造成持续重绘与抖动；
   主题偏好留在 `System` 却又强制 light visuals，两者互相覆盖。

## 修复

- `ui()` 用 `egui::CentralPanel` 包裹内容；`clear_color` 返回不透明的 `visuals.window_fill()`。
- `logic()` 每 100 ms 请求一次重绘；`WindowLevel` 只在真正变化时发送；主题用 `set_theme(Light)`
  固定。
- 附带：事件目录超过 500 个文件时改为「丢弃数量变化才 warn」。此前每个 tick 都写一条，日志被灌满，
  真实告警被淹没。

## 验证

- `cargo test`（41 项）、`cargo fmt --check`、`clippy -D warnings`、仓库守卫全部通过。
- 隔离台架：Xvfb `:99` + 单个 uinput 假键盘，并用 mount namespace 把 `/dev/input` 换成只含该假设备的
  tmpfs，全程不碰真实键鼠。把 Xvfb 根窗口涂成品红后，Stasis 窗口内品红像素占 0%（旧版本会大面积
  透出）；锁定、armed、解锁三个状态截图逐帧正确，且 armed 是在没有任何 X 输入事件的情况下出现的 ——
  正是旧版本不重绘的场景。

## 已知未修（需要决策）

- `is_ime_virtual()` 会跳过所有名字含 `uinput` 的设备，本机 `RustDesk UInput Keyboard`(event26) 因此
  永远不被抓取；若通过 RustDesk 远程操作本机，锁定时按键仍会进入系统。建议把过滤收窄到
  fcitx/ibus/`input method` 标记，本次未改。
- 抓取期间 KWin 的 kscreenlocker 读的是同一批 evdev 设备，理论上会出现「系统锁屏收不到输入」。未在
  真机复现，仅记录为风险。
- UI 仍没有渲染回归测试，本次靠台架人工核对截图。
