# Stasis 真实桌面事故排查：解锁不生效与重复 `rest.locked`

日期：2026-09-14。机器：KDE Plasma + Wayland，用户属 `input` 组。
触发场景：把 Stasis 接到 aide/rest-break 正在使用的事件目录后启动，用户被锁在
桌面外。结论：**事件读取窗口按文件名截断**是主因，另有两个同源缺陷。

## 现象与证据

启动（23:07:00）后，事件目录 `input-locker-events/` 中的实测序列：

- 23:07:01–23:07:07 连发 **8 条 `rest.locked`**，每秒一条，而不是一次锁定。
- 23:07:37–23:07:39、23:08:09–23:08:13 各出现连续多秒的 `rest.observed`，
  而心跳本应 30 秒一次。
- 23:09:07 写入命令文件 `unlock`，ack `ok`、`rest.unlocked(reason=command)`
  落盘；**23:09:08 又写出一条 `rest.locked`**，session 回到 active。
  用户仍被锁，只能停掉进程恢复。

## 根因

三个缺陷共享一条主线：投影每次读到的历史不是「全部事件」。

1. **事件窗口按随机文件名截断（主因）。**
   `src/protocol.rs` 中 `read_dir` 在目录超过 `MAX_READ_BATCH = 500` 时，
   保留「文件名排序最后的 500 个」。事件文件名是随机 UUID，因此实际保留的是
   **随机一半历史**。该目录当时有 1088 个 results 文件，于是：
   - 刚写入的 `rest.unlocked` 有一半概率对投影不可见 → session 仍为 active →
     下一 tick 由「计划休息已恢复锁定」分支重新抓取，**手动解锁永远不生效**；
   - 心跳节流依据 `segments.last()` 计算，segments 随可见事件随机变化 →
     30 秒节流失效，连续多秒补发 `rest.observed`。
2. **`rest.observed` 会复活已结束的 session。** `project` 对 `OBSERVED`
   无条件 `phase = Active`，即使该事件落在已 `unlocked` 的 segment 之后。
   这是 (1) 的放大器：即使解锁事件可见，一条迟到心跳也会把 session 拉回
   active。
3. **恢复锁定会伪造新 segment。** active session 每次发现物理未锁定就写一条
   新的 `rest.locked`。后端抓取抖动时（本机抓取线程在 7 秒内反复退出，
   原因见下），就退化成每秒一条锁定事件。

## 修复（`docs/` 之外，仅 `src/`）

- `protocol::EventStore::read_dir`：按 **mtime** 取最新 500 个，不再按文件名；
  扩展名过滤提前，减少无谓 stat。
- `session::project`：`OBSERVED` 只在确实延长了一个未关闭 segment 时才置
  `Active`，否则保持原 phase（`Ended` 不被复活）。
- `session::Controller::tick`：恢复已打开 segment 的锁时不再写新的
  `rest.locked`，只恢复物理抓取并给出状态消息。
- `engine::run`：抓取线程退出、后端 Health、以及「未报错就死亡」三条路径补
  `tracing::warn!`，避免下次只能靠事件文件反推。

回归测试：`newest_events_survive_the_batch_cap`、
`late_heartbeat_does_not_revive_an_ended_session`、
`backend_flap_resumes_without_a_duplicate_lock_event`。

## 验证状态

已验证：`cargo test`（39 项）、`cargo fmt --check`、
`cargo clippy --all-targets -- -D warnings`、仓库守卫；接入真实事件目录启动后
只写出一条正确的 `rest.unlocked(reason=failure)` 收尾旧 session，未再抓取输入。

未验证（不得视作通过）：

- 抓取线程为什么在 23:07:01–23:07:07 反复退出。本机 `/dev/input` 含多个
  可热插拔设备（Razer 无线鼠标 6 个 event 节点、`mouce-library-fake-mouse`
  等），23:07:28 出现 3 次 ungrab `ENODEV`，怀疑设备节点消失触发
  `devices.is_empty()`。属实施计划中「热插拔、半数设备抓取失败」未覆盖区，
  只能靠新加的日志在下次复现时定位。
- 完整休息周期（真实计划锁定 10 分钟 → 自动解锁）在修复后的端到端表现。
- 事件目录只增不减：本次靠 mtime 截断兜底，长期需要归档或裁剪策略。

## 后续项

- 事件目录增长与裁剪（当前 1088 个 results 文件，无清理机制）。
- `SIGTERM` 时 `tracing` 非阻塞 writer 不落盘，日志文件为 0 字节；systemd
  停止场景只能依赖 stderr。
- aide 侧 `internal/locker/events.go` 的 `rest.observed` 同样无「已结束」判断，
  建议同步修正（本仓库未改动 aide）。
