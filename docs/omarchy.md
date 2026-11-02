# Stasis 在 Omarchy 上的适配

> 状态：已在本机 Omarchy（Hyprland + Wayland）完成 daemon、CLI、bar widget 的
> 编译与界面验证；真实抓取/解锁待 `input` 组授权后复验（见「验证状态」）。

## 结论

Omarchy 用 Hyprland（Wayland）和 Quickshell 写的 omarchy shell。适配分两部分：

1. **锁定后端不用改**。Linux 后端直接对 `/dev/input/event*` 做 `EVIOCGRAB`，
   抓取发生在内核层、合成器之前；Hyprland 收不到被抓住的键鼠事件，所以
   Wayland 下同样能锁。前提是进程有 `input` 组权限。
2. **UI 换成状态栏组件**。不再依赖托盘或常驻窗口：`stasis daemon` 只跑引擎，
   把状态写进 JSON；omarchy shell 的 bar widget 读状态、通过 CLI 发起休息。

这样解锁仍然只依赖 evdev 手势与密码，与窗口焦点、shell 是否卡住无关，
符合 [design.md](design.md) 的既有契约。

## 组成

```text
omarchy/
  plugin/            bar widget（manifest.json + BarWidget.qml）
  stasis.service     systemd 用户单元，运行 stasis daemon
  install.sh         构建 + 安装二进制/单元/插件（无需 root）
src/status.rs        运行时状态文件
src/main.rs          daemon / status / rest / lock / unlock 入口
```

数据流：

```text
bar widget ──stasis rest N──▶ requests/rest.requested
     ▲                              │
     │                              ▼
 status.json ◀── engine tick ◀── Controller 投影
                                    │
                                    ▼
                          evdev grab / 定时解锁
```

发起休息走的是 aide 已在用的 `rest.requested` 事件，不新增控制协议；
状态文件只是只读投影，不参与解锁决策。

## 安装

```sh
cd <repo>
./omarchy/install.sh
```

脚本做四件事：`cargo build --release --bin stasis`、装到 `~/.local/bin/stasis`、
安装并启用 `stasis.service`、把 `omarchy/plugin` 复制到
`~/.config/omarchy/plugins/stasis.rest` 并启用 bar widget。

手动安装等价于：

```sh
install -Dm755 target/release/stasis ~/.local/bin/stasis
install -Dm644 omarchy/stasis.service ~/.config/systemd/user/stasis.service
cp -r omarchy/plugin ~/.config/omarchy/plugins/stasis.rest
systemctl --user daemon-reload && systemctl --user enable --now stasis.service
omarchy-shell shell rescanPlugins
omarchy plugin enable stasis.rest --section right
```

## 权限（必须）

抓取需要读 `/dev/input/event*`：

```sh
sudo usermod -aG input "$USER"
# 重新登录后生效
```

验证：`stasis status` 正常、且发起一次 1 分钟休息时状态从 `locked=false`
变成 `locked=true`。若日志出现 `no input devices found`，就是没进 `input` 组。

组变更对**已登录会话无效**：`systemd --user` 和它启动的 `stasis.service`
继承登录时的组集合，`systemctl --user restart` 也不会重新读取。加组后必须
重新登录（或注销再登录），再确认：

```sh
id -nG | tr ' ' '\n' | grep -x input   # 重新登录后应输出 input
systemctl --user restart stasis.service
stasis status
```

安装脚本不会自动改用户组，也不会在安装时抓取输入——daemon 启动后是空闲状态。

## CLI

| 命令 | 作用 |
| --- | --- |
| `stasis daemon` | 只跑引擎，无窗口、无托盘（systemd 单元用这个） |
| `stasis status [--json]` | 打印状态文件；daemon 不在时退出码 1 |
| `stasis rest [N]` | 写入 N 分钟（默认 20，上限 240）的休息请求 |
| `stasis lock` / `stasis unlock` | 走既有 command-file 协议，等待 ack |
| `stasis afk` | 读 evdev（不抓取）向 ActivityWatch 发布 AFK 心跳，供工作时长显示 |
| `stasis autorest` | rest-break 判定 due 时写一条休息请求（给每分钟的 timer 用） |

`stasis rest` 在 daemon 不存活时拒绝执行（避免请求积压到下次启动才锁），
调试可加 `--force`。

## 状态文件

位置：`$XDG_STATE_HOME/stasis/status.json`，默认
`~/.local/state/stasis/status.json`；`STASIS_STATE_DIR` 可覆盖。引擎每秒
原子替换一次，权限 `0600`。

```json
{
  "schema": "stasis.status/v1",
  "running": true,
  "pid": 1234,
  "locked": true,
  "unlockMode": false,
  "passwordLen": 0,
  "message": "已锁定",
  "ok": true,
  "updatedAt": "2026-10-31T06:13:33.371Z",
  "eventsDir": "/home/you/.local/share/stasis/events",
  "session": {
    "sessionId": "6c1d…",
    "phase": "active",
    "lockAt": "2026-10-31T06:13:30.942Z",
    "unlockAt": "2026-10-31T06:14:30.942Z"
  },
  "lastRestEnd": "2026-10-31T06:14:30.942Z"
}
```

读者必须用 `updatedAt` 判断存活（超过 5 秒视为 daemon 已退出）：进程被
`SIGTERM` 杀掉时不会补写 `running=false`。

## bar widget

- 左键打开面板，选择 5/10/20/30 分钟；点击后调用 `stasis rest N`。
- 锁定期间 bar 只显示倒计时；键鼠已被抓取，点击无效是预期行为。
- 失败时图标变 urgent 色，悬停提示查看 `stasis status`。
- 面板底部提示提前解锁手势：连按 3 次 `j`，再输入密码回车。
- 空闲时 bar 显示当前连续工作时长（如 `1h12m`），面板显示 `WORKING` /
  `NEXT REST`；没有数据时只显示图标。时长优先用 status 文件的
  `lastRestEnd` 计算（距上次休息结束），休息结束后从 0 重新计时；没有休息
  记录时才回退到 rest-break 的 3 小时滚动窗口值。
- 插件设置：`binary`（默认 `~/.local/bin/stasis`）、`durations`
  （逗号分隔）、`statusFile`（默认见上）、`restBreakStatusFile`
  （可选，默认自动探测）。
- IPC：`omarchy-shell stasis.rest open|close|toggle|refresh|rest <N>`。

工作时长有两个来源，互不阻塞：

- 主来源是 Stasis 自己的 status 文件：daemon 从事件投影里取最近一次
  `rest.unlocked` 的 `unlockedAt`，写进 `lastRestEnd`，bar 用它算“距上次
  休息结束”。这样休息完时长会归零重算，不会把休息前的工作继续累加。
- 回退来源是 rest-break 的 `status.json`（3 小时滚动窗口内的工作分钟），
  仅在没有 `lastRestEnd` 时使用。

- 探测顺序：`restBreakStatusFile` → `$XDG_STATE_HOME/rest-break/status.json`
  → `~/stasis/rest-break/status.json` → `~/stasis/rest-break/target/release/status.json`。
- 只接受 `checkedAt` 在 3 分钟内的数据（rest-break 每分钟写一次）；过期
  视为不可用，不显示旧数字。
- rest-break 依赖 ActivityWatch（`aw-server`，默认 `localhost:5600`）提供
  工作/离开数据；ActivityWatch 或 aide 不可用时 rest-break 写
  `state=error`，bar 只是不显示工作时长，**不影响** `stasis rest N` 与
  锁定/解锁。

## 与 rest-break 的关系

两者共用事件目录：rest-break 通过 aide 写 `rest.requested`，Stasis 投影后
执行锁定与定时解锁。Linux 默认事件目录为
`$XDG_DATA_HOME/stasis/events`（默认 `~/.local/share/stasis/events`）；若
rest-break/aide 用别的目录，在 `config.json` 的 `events_dir` 显式对齐。
同一个目录只能有一个消费者（不要和旧 Python 程序共用）。

### 工作时长数据链（可选）

bar 空闲时显示的 `1h12m` 在回退路径上来自 rest-break：

```text
stasis afk ──evdev 只读──► aw-server(-rust) :5600 ──► rest-break --check
                                                          │
                                        ~/.local/state/rest-break/status.json
                                                          │
                                    ┌─────────────────────┴──────────────┐
                                bar widget                        stasis autorest
                                                                        │
                                              rest.requested ──► stasis daemon ──► 锁定
```

`stasis autorest` 是补齐调度的一环：本机 aide 只暴露 `get_locker_plan` /
`set_locker_plan`，没有 rest-break 调用的 `request_rest` / `get_rest_sessions`，
所以 rest-break 的调度路径在这里会失败。`autorest` 不改模型，只读 rest-break
已经写好的 `due` / `restMinutes`，检查 daemon 存活、没有进行中的休息、距上次
休息超过冷却（默认 30 分钟），然后写一条与 `stasis rest` 相同的
`rest.requested`。`stasis-autorest.timer` 每分钟跑一次。

- 为什么不用官方 `aw-watcher-afk`：它用 pynput 读 X11，Hyprland 下看不到
  输入，会一直报「活跃」。`stasis afk` 读同一批 evdev 设备，不需要抓取，
  只要 `input` 组读权限。
- 服务端用 aw-server 或 aw-server-rust 都行（同一个 `/api/0` 接口）。
  aw-server-rust 可用 AUR `aw-server-rust-git`，或
  `git clone https://github.com/ActivityWatch/aw-server-rust && cargo build --release`
  后把 `target/release/aw-server` 装成 `~/.local/bin/aw-server-rust`。
- `rest-break --check` 只读 ActivityWatch、不经过 aide，因此只刷新状态文件，
  不会安排休息；用 `rest-break-check.timer` 每分钟跑一次。
- 安装：`./omarchy/install.sh --with-afk` 会构建并启用 `stasis-afk.service`
  和 `rest-break-check.timer`。ActivityWatch 服务本身需自行安装，本仓库
  不附带第三方单元。
- 停掉这条链只影响工作时长显示；`stasis rest N` 和锁定/解锁完全不受影响。

## 已知限制

- 不覆盖屏幕。锁定期间桌面仍可见，bar 的倒计时是唯一反馈；若需要全屏提示，
  应另做 layer-shell overlay，而不是恢复 egui 窗口（Wayland 下无法置顶）。
- 没有“开始前取消”。点击即写入请求，引擎在 1 秒内锁定；提前结束只能用手势
  或等定时解锁。
- 面板里的 IPC 每个屏幕一个实例，Quickshell 会打印
  `another handler is registered for target stasis.rest`；热键 summon 只会命中
  其中一个屏幕，鼠标点击不受影响。
- `omarchy plugin update` 只管理 git 安装的插件；本插件由 `install.sh` 复制，
  更新仓库后需重跑脚本。
- 直接改 `~/.config/omarchy/plugins/` 里的 QML 会触发热重载，但已经渲染的
  bar 实例可能仍用旧组件（`IpcHandler` 行号停在旧版本即可判断）。改完插件后
  执行 `omarchy-restart-shell` 才能确保新代码生效。

## 验证状态

本机（Omarchy，Hyprland/Wayland）已验证：

- `cargo test` 58 项、`cargo clippy --all-targets -- -D warnings`、
  `cargo fmt --check` 通过。
- daemon 启动、状态文件写入、`stasis status --json`、`stasis rest`、
  `stasis lock` 的 ack/失败路径。
- bar widget 加载、空闲/失败两种图标状态、面板渲染、IPC 调用
  `stasis.rest rest 1` 触发完整链路。
- 失败重试退避：抓取失败时 10 秒一次，不再每 tick 写 `rest.failed`。

- 2026-10-31 真实 `EVIOCGRAB` 锁定（已授权，临时 daemon + 临时状态/事件
  目录，未动系统服务）：`input` 组下 12 秒会话，第 1 秒 `locked=true`、
  `phase=active`、消息「计划休息已锁定」，第 12 秒自动解锁为
  `locked=false`「计划休息完成」，daemon 无错误日志。测试用 watchdog 兜底，
  结束后已删除临时目录。

未验证：

- `j`×3 + 密码手势在 Omarchy 上尚未复验（Linux 端在 KDE 环境验证过，
  见 [README](../README.md)）。
- 本机 `stasis.service` 仍运行在加组前的登录会话里，需重新登录后才具备
  `/dev/input` 读权限；上面的抓取测试是用 `setpriv --init-groups` 起临时
  daemon 完成的。

## 故障排查

```sh
stasis status --json
systemctl --user status stasis
journalctl --user -u stasis -n 50
omarchy plugin list | grep stasis
omarchy-shell shell rescanPlugins
```

插件改了代码不生效：先确认 `~/.config/omarchy/plugins/stasis.rest/` 是复制
目录（不是指向仓库的软链），必要时 `omarchy restart shell`。
