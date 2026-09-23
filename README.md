# Stasis

Stasis Domain

**Pause the machine. Rest the human.**

Input Locker 的 Rust 重写项目：暂停键盘、鼠标输入，让人休息。

目标平台：Windows、Linux（X11 / Wayland）、macOS。核心原则：**行为定义一份，平台只适配输入与系统
资源；
解锁不依赖窗口焦点或 UI 响应。**

## 当前状态

Linux 端已实现并在授权真实桌面（KDE Plasma + Wayland）完成端到端验证。Windows 输入后端已实现，
并在真机（Windows 11、console 会话、进程不提权）完成锁定/解锁主链路验证（见下）；macOS 尚未
实现，不作为已验证能力声明。

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

### Windows 已验证（2026-09-20，真机）

- 环境：物理机 Windows 11（build 10.0.26200），console 会话、进程不提权；受测构建
  `stasis.exe` sha256 `ed5882be…8ddd07`（commit `02243e4`，`x86_64-pc-windows-gnu`）。
- 锁定：UI「锁定系统」与 command 文件 `{"cmd":"lock"}` 均能落锁；低层鼠标钩子确实吞掉
  指针——锁定中注入鼠标移动，光标位移 0 px。
- 解锁：真键盘 2 秒内连按 3 次 `j` 进入解锁模式（日志 `unlock mode armed by gesture`），
  密码 + 回车后 UI 显示「已成功解锁」，钩子随即释放（解锁后注入的同一移动位移 376 px）。
  **注意**：leg 1 期间「成功解锁」没有日志行能区分「密码解开了」还是「回车把 UI 按钮激活了」；
  同日修复复验才第一次拿到 `unlocked (password)`（见下）。
- command 文件 `{"cmd":"unlock"}` 可强制解锁（本轮两次用于恢复，都在 session 0 执行，
  不受 session 1 钩子影响）。注意命令 record 必须带 `id`（`{"cmd":"lock","id":"…"}`）：
  缺 `id` 时应用会静默丢弃命令，既不回 ack 也不写日志。
- 完整人工流程（人手走完）：点「锁定系统」→ 2 秒内连按 3 次 `j`（日志
  `unlock mode armed by gesture`）→ 手输密码 + 回车 → UI 未锁定；吞没探针由 0 px
  变为 470 px。物理按键到达钩子的原始证据：`scan=36 flags=0`（注入键是 `scan=0
  flags=16`）。
- 测试方法说明：目标机的**键盘挂在一个 USB 切换器后面**（鼠标常驻目标机）。切换器把键盘
  切到别的机器时，目标机收不到任何键盘输入——早前几次「按了没反应」有一部分是它；但同一个
  症状还有更根本的原因（见下条），不能再一律归到这个切换器。
- **锁定窗口不得占据前台（修复，同日真机复验）**：Windows 在**本进程自己的窗口占据前台**时
  不再调用低级键盘钩子——钩子仍装着（`UnhookWindowsHookEx` 仍成功），但键直接落到该窗口，
  引擎一个字节都收不到。「`j`×3 能进解锁模式、之后输密码无效、必须 `Tab`+`Enter`」就是它。
  修复：arming 不再抢前台（窗口仍 `AlwaysOnTop` 可见），安装钩子时把前台让给 z-order 里下一个
  非本进程窗口；锁定期间若有键落到窗口（说明捕获失效），UI 要求引擎重建钩子并清空已输密码，
  重建失败则解除锁定、不假装锁着。复验三条路径（UI 按钮锁定 / command 文件锁定 / 强制制造泄漏）
  都得到 `unlocked (password)`、无泄漏告警；受测构建 sha256 `7aa064b0…44613a`。
- 锁定约 25 分钟内日志无 `Health` 与回调耗时告警，但**这个信号不足以判断捕获是否有效**：
  「窗口占前台」那次失效期间同样没有任何 `Health`（回调根本没被调用）。
- **2026-09-23 最小化窗口修复（真机复验）**：自动锁定前窗口已最小化时，旧版仅置顶，
  用户看不到解锁模式与密码圆点。新版锁定时恢复可见、取消最小化，仍不抢前台；
  先最小化再锁定的命令路径、自动休息路径均以真键盘 `j`×3 → 密码 → 回车解锁，
  自动休息写入 `rest.unlocked`（`password`）。受测 Windows 构建 sha256
  `3e79189d…2e4918`。此前单次密码错误的具体字符成因无法从日志还原，
  不将其等同于钩子丢键。证据见
  [2026-09-23 排查与复验](docs/notes/windows-unlock-incident-2026-09-23.md)。

完整测试记录与截图见
[docs/notes/stasis-windows-realtest-leg1.md](docs/notes/stasis-windows-realtest-leg1.md)。

### 尚未验证 / 未来工作

- **Windows 仍未覆盖**：钩子被静默移除的检测路径（watchdog / `Released`）真机从未触发；
  UIPI 边界（发往提权进程与 UAC 安全桌面的输入看不到也吞不掉）在测试机上没有提权用户进程，
  仍未实测；Ctrl+Alt+Del 系统出口无法用注入方式验证；单次事件未被钩子调用（真机记录
  Finding 5 那次单键）仍未归因。**交叉编译通过不等于功能验证。**
- **已知交互缺口（真机实测）**：锁定中「强制解锁（UI）」不可达——鼠标点击被鼠标钩子吞掉，
  键盘修复后又被正常捕获（`Tab` 被当未映射键吞掉、`Enter` 变成提交密码），所以锁定期间的
  应用内通路就是密码；进程外恢复靠 command 文件或终止进程。详见真机记录「Findings」。
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

### rest-break 状态 UI（`rest-break-ui`）

`rest-break/src/bin/rest-break-ui.rs` 是常驻状态小窗：一行为摘要（疲劳分钟 · 下次休息 ·
时长），指针移入即展开完整字段（state、fatigueMinutes、nextRestAt、nextRestMinutes、
workMinutes、circadian、reason、checkedAt、cooldownUntil），移出 0.6s 后收起，点击可
钉住详情（再点一次取消）。它只读 status.json（默认每 1s 重读），不写判定状态、不抓输入；
崩溃或退出均不影响判定与锁屏链路。GUI 依赖复用根 crate 的 eframe/egui 0.36，放在
optional feature `ui` 下，因此 cron 二进制的默认依赖树不含 GUI：

```sh
cargo run --manifest-path rest-break/Cargo.toml --features ui --bin rest-break-ui
bash rest-break/ui_interaction_test.sh  # 隔离 Xvfb + XTEST 真指针事件的交互检查
```

配置经环境变量：`STATUS_FILE`（缺省 `./status.json`）、`RB_UI_CORNER`=`ne|nw|se|sw`
（缺省 `se`）、`RB_UI_MARGIN`（缺省 `90`）、`RB_UI_FONT_SIZE`（缺省 `14`）、
`RB_UI_TEXT_COLOR`（`#rrggbb`，缺省白）。

验证状态（真机为 Linux XWayland 授权桌面）：窗口可见、右下角定位与展开越界回弹、置顶
（`_NET_WM_STATE_ABOVE`）、随 status.json 刷新、错误态如实提示、只读不改生产 status.json、
退出后无残留进程，均已实际确认。悬停展开、移出 0.6s 宽限收起、点击钉住/取消三态由
`rest-break/ui_interaction_test.sh` 在隔离 Xvfb（真 X 服务器）上用 XTEST 真指针事件逐步
核对通过并留下截图，状态机另有 3 个离线单测。**未验证**：真实桌面会话里的真鼠标交互——
KWin/XWayland 不把 `xdotool` 注入的指针运动投递给客户端（在 `:1` 上用修复后的代码重测：
指针进入窗口后窗口几何不变），需人工用真鼠标确认一次：

1. 启动：`cargo run --manifest-path rest-break/Cargo.toml --features ui --bin rest-break-ui`
   （`STATUS_FILE` 指向 status.json，缺省 `./status.json`）；
2. 指针移入小窗 → 展开九个字段；移出约 0.6s → 收起；点击一次 → 移出后仍展开；
   再点一次 → 移出后收起；
3. 证据：展开态与钉住态截图（或录屏）+ 观察结论；退出后 `pgrep -x rest-break-ui` 无残留。

Windows 仅 `cargo check --features ui --target x86_64-pc-windows-gnu` 通过，未做真机运行。
Wayland 原生协议无全局置顶能力，置顶依赖 XWayland/EWMH。

## 规范

- 提交信息、分支命名、AI 声明：[CONTRIBUTING.md](CONTRIBUTING.md)（提交信息统一英文）。
- Agent 约定、文档规则、禁区：[AGENTS.md](AGENTS.md)。
- 文档索引与分类：[docs/README.md](docs/README.md)。

克隆或新建 worktree 后先安装钩子（`core.hooksPath` 是本地配置，不随 clone 传播）：

```sh
git config core.hooksPath .githooks
```
