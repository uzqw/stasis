# Stasis Linux — VM evdev verification record (leg 2)

Date: 2026-09-14. Environment: `virtme-ng --disable-microvm`,
host-compiled release binaries mounted into VM.

## 变更摘要

- `classify()` 改用 KEY_SPACE + KEY_ENTER 做键盘检测，覆盖更多非标准键盘。
- 新增 `is_ime_virtual()` 跳过名称含 fcitx/ibus/input method/uinput 的虚拟设备，
  避免 Wayland 下抓取 IME 虚拟键盘导致输入法中断。
- `scan_new()` 不再静默跳过失败，而是返回错误列表，由后端线程通过
  `BackendEvent::Health` 上报。
- 新增 `BackendEvent::Health(String)`，引擎在收到后端健康事件时更新 UI 消息。
- 后端线程在设备读取报错时发送 Health 事件，实现逐设备健康监控。

## VM 设备列表

| 设备 | 名称 | 分类结果 |
| --- | --- | --- |
| event0 | Power Button | 跳过（无打字键） |
| event1 | AT Translated Set 2 keyboard | Keyboard，已抓取 |
| event2 | PC Speaker | 跳过（无打字键/指针） |
| event3 | VirtualPS/2 VMware VMMouse | Pointer，已抓取 |
| event4 | VirtualPS/2 VMware VMMouse | Pointer，已抓取 |

## 验证结果

| 用例 | 结果 |
| --- | --- |
| grab_test 自动抓取/5s 释放 | PASS |
| unlock_test 启动抓取/timeout 退出 | PASS |
| cargo test (debug, VM 内) | 35/36 PASS；1 个已知 root 权限相关失败 |

## 已知限制

- `event_write_failure_rolls_back_lock` 在 VM root 环境下因 root 可绕过只读权限而失败，
  属测试前提假设问题，非实现缺陷。宿主非 root 运行 36/36 PASS。
- VM 内无图形，完整 UI 锁—Caps×3—密码—解锁 流程已在授权真实桌面验证（leg-1 记录）。
- 热插拔 `scan_new()` 的 Health 上报在 VM 内未触发（无物理热插拔事件），
  代码路径通过编译与单测覆盖。
