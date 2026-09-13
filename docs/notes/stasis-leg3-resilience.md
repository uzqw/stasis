# Stasis — UI freeze resilience & non-blocking I/O (leg 3)

Date: 2026-09-14. 变更范围：`protocol.rs`、`session.rs`、`engine.rs`。

## 变更摘要

- `EventStore::read_dir` 新增 `MAX_READ_BATCH = 500` 上限，防止异常大量
  事件文件拖慢读取。
- 新增 `EventStore::events_nonblocking(timeout)`：在非测试环境下使用独立线程
  读取事件并在超时时返回空列表，避免引擎线程被慢速存储永久阻塞。
- `Controller::sessions()` 在生产环境调用 `events_nonblocking(Duration::from_secs(2))`；
  测试环境保持同步路径，确保现有单测行为不变。
- `engine.rs` 中 `poll_command` 从每秒执行降为每 5 秒，减少文件 I/O 对引擎
  select 循环的阻塞面。

## 设计依据

引擎线程使用 `crossbeam_channel::Select` 同时监听 UI 命令、定时 tick 和后端
输入事件。如果 `tick` 分支中的同步文件 I/O（`events()` / `poll_command`）阻塞，
键盘解锁事件和到期解锁都会延迟处理。

`events_nonblocking` 的超时降级策略：
- 正常情况（本地文件系统）`events()` 在毫秒级完成，超时几乎不触发。
- 若超时返回空列表，`unlock()` 会看到 `active.is_empty()`，仍然执行
  `locker.stop_lock()` 物理解锁，不会困住用户。
- `tick()` 超时跳过本次调度，1 秒后重试，累积延迟可接受。

`poll_command` 降频到 5 秒：command/ack 是 aide 侧低频管理命令，不需要
秒级响应；降低频率减少阻塞面。

## 验证结果

| 检查项 | 结果 |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --all-targets -- -D warnings` | PASS |
| `cargo test` | 36/36 PASS |
| Guard | PASS |

## 已知限制

- `events_nonblocking` 每次调用创建临时线程；极端慢存储下可能累积后台线程。
  这是 Rust std 无原生异步文件 I/O 的折中，实际本地存储场景无影响。
- 真机 UI 冻结 + 慢存储组合未在 VM 内复现；依赖架构审查和代码路径分析。
