# Stasis

Stasis Domain

**Pause the machine. Rest the human.**

Input Locker 的 Rust 重写项目：暂停键盘、鼠标输入，让人休息。

目标平台：Windows、Linux（X11 / Wayland）、macOS。核心原则：**行为定义一份，平台只适配输入与系统
资源；
解锁不依赖窗口焦点或 UI 响应。**

## 当前状态

Linux 端已实现并在授权真实桌面（KDE Plasma + Wayland）完成端到端验证；Windows 与 macOS 尚未实
现，属于未来工作，不作为已验证能力声明。

解锁手势（2026-09-14 起）为 **2 秒内连按 3 次 `j`**；下文 2026-09-13 记录中的 CapsLock×3 是当时的
行为，已不再生效。

- [设计方案](docs/design.md)：范围、跨平台行为、线程与资源模型、协议兼容、安全边界。
- [实施与验收计划](docs/implementation-plan.md)：按可验证成果推进，不按代码量判定完成。
- [决策记录](docs/adr/0001-rust-rewrite-shared-core.md)：为什么重写、为什么是共享核心结构。

### Linux 已验证（2026-09-13，真实桌面）

- UI 正常启动渲染，中英文显示正常（nested KWin 内截图验证）。
- evdev 真实抓取：锁定后真实键盘/鼠标输入被吞没，解锁后恢复。
- CapsLock×3（2 秒内）进入解锁模式，正确密码解锁；错误密码提示并可重试。
- 窗口外 Caps、长按自动重复不误触发；锁定→解锁→再锁定、锁定中 `kill` 均能干净
  释放。
- aide command/ack 文件协议（`input-locker-command.json` 等）轮询与应答正常。

完整测试记录与截图见
[docs/notes/stasis-linux-realtest-leg2.md](docs/notes/stasis-linux-realtest-leg2.md)。

### 尚未验证 / 未来工作

- **Windows、macOS**：无实现、未验证。
- Linux 端尚未覆盖的回归用例（热插拔、半数设备抓取失败、时钟跳变等）见
  [implementation-plan.md](docs/implementation-plan.md) 的回归用例表。
- 安装包、自启动、签名/权限引导等交付项（阶段 4）未完成；当前仅有 `cargo build --release`
  产物。

### 构建与运行（Linux）

```sh
cargo build --release
./target/release/stasis
```

需要 `input` 组权限以抓取 `/dev/input/event*`，密码规则为物理 US 键盘 ASCII 字母/数字。
真实抓取属于高风险操作：仅在授权桌面运行，并先确认恢复通道（uinput 注入、`kill`、UI 强制解锁
按钮）。

排查输入/锁定问题时先看应用自己的日志：`~/.config/stasis/stasis.log.YYYY-MM-DD`（按 UTC 日期滚动，
**不写 journald**）；需要更详细的等级时给进程设 `RUST_LOG=info`。

新项目独立开发，原 `../input-locker` 保留使用。切换前不得让两个程序同时消费同一事件目录或抓取输
入。

## Rest-break 判定核心

`rest-break/` 是独立 Rust crate，负责「何时休息」，不依赖锁屏 GUI 或输入后端。
已移植疲劳积分、昼夜节律、冷却与实际锁定区间恢复，以及 AW 拉取、aide MCP 直连、
pending 防重与 status.json 输出的运行时编排；44 个离线测试对应 Go 判定与编排测试。
复用 chrono 处理带时区时间、serde/serde_json 处理协议 JSON；HTTP 仅打本机/内网
（AW :5600 与 aide MCP），选默认特性全关的 ureq（无 TLS/gzip）保持最小依赖；
tempfile 仅用于隔离测试。独立 crate 避免每次构建 cron 工具都引入 GUI 和输入设备依赖。

```sh
cargo test --manifest-path rest-break/Cargo.toml
cargo fmt --manifest-path rest-break/Cargo.toml --check
cargo clippy --manifest-path rest-break/Cargo.toml --all-targets -- -D warnings
```

二进制已实现完整编排：`--check` 只判定不安排，due 时经 `request_rest` 提交
rest.requested 事件（不写 history），pending.json 提供 35 分钟不确定执行保护。
`rest-break/run_rest_break_cron.sh` 是 cron 每分钟入口（flock 防重、日志滚动、
WSL 网关自动发现、overlay 文本生成）；`rest-break/integration_test.sh` 在隔离
目录里跑端到端：mock AW + 隔离 aide + 真 rest-break 二进制 + Stasis session
controller（`examples/rest_session_drive`，假锁执行器），全程不碰生产服务与
真机输入。已验证通过一次；切换/替换现役 aide 部署仍由用户另行决定。
根目录 Cargo 命令不包含此独立 crate，须额外执行上面的检查。

## 规范

- 提交信息、分支命名、AI 声明：[CONTRIBUTING.md](CONTRIBUTING.md)（提交信息统一英文）。
- Agent 约定、文档规则、禁区：[AGENTS.md](AGENTS.md)。
- 文档索引与分类：[docs/README.md](docs/README.md)。

克隆或新建 worktree 后先安装钩子（`core.hooksPath` 是本地配置，不随 clone 传播）：

```sh
git config core.hooksPath .githooks
```
