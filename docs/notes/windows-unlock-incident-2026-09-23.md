# Windows 自动锁定后无界面：2026-09-23 排查与复验

## 结论

本次「按 `j`×3 后无界面」不是已证实的钩子丢键：手势到达引擎，随后有密码提交。
**确定的故障是锁定窗口在锁定前已最小化，旧版锁定时只置顶，不恢复最小化。**
用户看不到解锁模式和密码圆点；本次错误密码的具体字符是否遗漏则无法从隐私安全的日志判定。

修复后在授权 Windows 11 真实桌面，重现「先最小化，再锁定」并完成两次
`j`×3 → 密码 → 回车解锁（包括一次计划休息自动锁定）；不需要 Stasis 占据前台。

## 事故证据（UTC）

- `02:27:42` 计划休息写入 `rest.locked`；`02:28:13` 日志为
  `unlock mode armed by gesture`，证明三次 `j` 到达引擎。
- 直到 `02:40:45` 才有 `unlock rejected: wrong password`，证明至少一次提交到达引擎；
  不记录密码内容，无法还原此前键入内容或逐键是否遗漏。
- `02:40:45` command 文件 `unlock` 获得 `ok` 回执；日志 `unlocked (command)`，
  对应会话写入 `rest.unlocked`。恢复通道在真实桌面可用。
- ActivityWatch 的窗口事件显示 `02:26:07` 起约 880 秒前台为非 Stasis 进程；
  早前两次密码成功解锁时前台也不是 Stasis。这反驳了**本次**「Stasis 占据前台」
  的猜测，但前台记录不证明窗口是否可见。
- `rest.observed` 与目录裁剪告警持续；无证据表明 controller tick 停止。
  手势后也没有触发 3 秒 armed-silence 告警，不能反推每个密码字符都到达。

## 真机对照与修复验证（UTC）

诊断任务在交互会话 1 只读取本进程顶层窗口的可见、最小化和前台状态，
不读取标题、屏幕或按键内容。所有真机锁定前均确认 command 文件恢复通道。

1. 旧版 `03:31` 未锁定时 `minimized=1`；`03:32:50` 命令锁定后
   `03:32:52` 仍为 `minimized=1`。手势于 `03:32:53` 到达；
   `03:32:55` 用 command 文件解锁。置顶并没有使窗口显现。
2. 修复版 `03:35:53` 经 `StasisLocker` 计划任务在会话 1 启动。
   `03:37:24` 先最小化再命令锁定，UI 日志记录 `minimized=Some(true)`、
   `focused=Some(false)`；`03:37:26` 探针显示 `minimized=0`，且前台不属于 Stasis。
   同轮手势、首次密码输入到达引擎，`03:37:28` 日志为 `unlocked (password)`。
3. `03:39:11` 提交短期 `rest.requested` 后自动写入 `rest.locked`。
   锁定时再次记录 `minimized=Some(true)`、`focused=Some(false)`；
   `03:39:14` 手势及首次密码输入到达，`03:39:15` 日志 `unlocked (password)`，
   同一会话写入 `rest.unlocked`，原因 `password`。兜底 command 后续回执 `ok`，
   但这次真正结束锁的是用户密码，不是兜底 command。

`Stop-ScheduledTask` 单独调用未及时终止旧 GUI。部署时先以 command 确认解锁，
再停任务、结束旧会话 1 进程，替换二进制并重新从计划任务启动。
原文件保留在目标机作为回滚副本。真机诊断任务、临时脚本与测试 exe 已移除。
本机 50 项 Rust 单测和 7 项守卫测试通过。

Windows 原生单测（2026-09-23 复验，release 构建）：

- `cargo test --release --no-run --target x86_64-pc-windows-gnu` 的产物拷到真机运行，
  49 项全部通过（`EXIT:0`）。跨编译不等于验证，这一步才是 Windows 侧单测证据。
- **debug 构建的测试可执行文件无法启动**：启动即 `0xC0000005`（会话 0 与会话 1 同样崩，
  `--list` 也崩）。原因不在会话/控制台、不缺 DLL，而是该产物有 20 个**孤儿 IAT 槽**
  （`GetModuleHandleA`、`Sleep`、`HeapAlloc` 等启动期函数未被任何导入描述符覆盖），
  加载器不绑定它们，调用时跳到 RVA 值（异常地址 `0x2fdd174` 正是 `GetModuleHandleA`
  的 hint/name RVA）。同工具链、同 target 的最小 crate 与 release 产物均无孤儿槽，
  故这是 debug/ld 侧问题，不是本项目的运行时代码缺陷，亦未经真机验证为可修。
- 修复前 release 测试在 Windows 有 3 项失败（`event_write_failure_rolls_back_lock`、
  `scheduled_unlock_event_write_failure_still_releases_lock`、
  `unlock_event_write_failure_still_releases_lock`）：它们用「把目录设只读」注入写失败，
  而 Windows 下目录只读不阻止在其中建文件。改为在 results 目录位置放一个同名文件
  （`events()` 同样读 `requests`，故已有事件移过去后会话状态不变），跨平台生效。

真机交互复验（子会话执行，2026-09-23T13:55Z–14:02Z）：命令锁定与自动休息锁定各一轮，
锁定前均先最小化窗口，日志均记录 `minimized=Some(true)`，随后探针 `minimized=0`，
全程 `foreground_is_stasis=False`；`j`×3 → 密码 → 回车得到 `unlocked (password)`，
计划释放写入 `rest.unlocked reason=scheduled`；结束后 22 个会话无未闭合 `rest.locked`。

**诊断盲区（未修）**：计划到点自动释放只在事件库留下 `rest.unlocked reason=scheduled`，
日志侧没有任何行（`计划休息完成` 只进 UI 快照）。排查时必须查事件库，
不能因日志无行而判断未解锁。命令文件锁定同样不写任何协议事件，只回 ack。

**未验证**：UAC 安全桌面与提权进程的输入边界；钩子被系统静默移除的真实触发；
本轮手势与密码均为注入键，未用物理键盘复验；debug 产物孤儿 IAT 槽的成因。

## 尚未证明

无法仅凭最小化窗口推断事故中密码错误的具体成因；若修复后再次出现密码错误，
应检查物理键盘所属机器、Shift/keyUp、回调链和输入队列，禁止记录明文密码或逐键内容。
真机测试仅覆盖当前桌面和登录会话，不证明 UAC 安全桌面或提权进程的输入边界。
