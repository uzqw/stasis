# macOS 自动休息调度修复（2026-09-30）

结论：三类缺陷已修并在真机验证其中两类；短时锁定/解锁链路在真机通过。
恢复自动调度后的一次完整 15 分钟自动休息未观察到结果——Mac 在预计锁定
时间前后离线，见「未完成项」。

## 现象

- 疲劳远超 90 分钟仍不安排休息，状态窗口却显示 `ok`。
- 一次失败请求后，35 分钟内不再重试，状态原因写作「防重」。
- 状态摘要用北京时间、详情用 Mac 本地时区，同一时间显示不一致。

## 根因

1. **aide 事件目录不可写**：launchd agent 缺少 `LOCKER_CONFIG`，cwd 为 `/`，
   事件写入 `/reports/input-locker-events`（只读文件系统）；Stasis 读的是
   `~/Downloads/input-locker-events`。修法：plist 补 `LOCKER_CONFIG` 与
   `WorkingDirectory=HOME`，`locker.json` 的 `schedule_file` 指向
   `~/Downloads/input-locker-plan.json`（事件目录取 plan 同级的
   `input-locker-events`）。
2. **MCP 工具错误被当成成功**：`parse_mcp_result` 只看 `result.content`，
   忽略 `result.isError`，于是真实失败被报成「响应无 sessionId」。
3. **失败判定与会话标识不匹配**：`get_rest_sessions` 的会话标识字段是
   `id`，不是请求参数里的 `sessionId`；按错字段比对导致失联请求永远
   无法确认，pending 只能等 35 分钟过期。

## 改动（rest-break）

- `parse_mcp_result` 检查 `isError`（JSON 与 SSE 两条路径）。
- `execute_rest_request` 先做无副作用的预检，再写 pending；失败后按稳定
  会话 ID 查询确认：已发布则清理 pending 并按实际 `lockAt` 返回，明确拒绝
  （工具错误且查询成功）才立即解除保护，传输/解析不确定仍保留 35 分钟并
  记录 `last_error`。
- 会话标识统一经 `session_key` 读取 `id`（兼容 `sessionId`）。
- `run_with` 展示真实在途会话（`waiting`/`active`）而非疲劳预测；`--check`
  不再删除已确认的 pending；观测错误不被 pending/会话状态覆盖。
- 状态新增 `scheduled`/`active`/`blocked`/`error`；`nextRestAt` 只在预计或
  已安排时填写，请求未确认时留空。
- UI 时间统一本地时区，摘要区分「预计 / 已安排 / 正在休息 / 请求未确认」，
  原因文案改为可读中文，不再出现「防重」。

## 真机验证（Mac，arm64，macOS 26.5.1）

调度暂停期间手工提交一次 10 秒会话，时间线（UTC）：

```text
16:54:16.99  request_rest(lockAt=16:54:24.99, unlockAt=16:54:34.99)
16:54:25.35  rest.locked    (lockedThrough 16:54:25.35)
16:54:34.99  rest.unlocked  (reason=scheduled)
```

锁定生效、按 `unlockAt` 自动解锁，事件落在 `~/Downloads/input-locker-events`。
辅助功能授权可用，无需用户再授权。

- Linux：`rest-break` 65 项测试、`clippy --features ui --all-targets
  -D warnings` 通过。
- Mac：同一套测试 65 项通过，含 macOS 字体回归项。

## 未完成项

- 恢复每分钟调度后，预计 17:10Z 左右自动安排一次 15 分钟休息；17:13Z 起
  Mac 在 10.0.0.9（WireGuard）不可达，未确认该次 `rest.requested` 与
  `rest.locked`。
- 冷却期是常量：任何已解锁会话都会让下一次休息推迟 15 分钟，10 秒的验证
  会话同样触发 15 分钟冷却。
- 上述两点都不改变修复方向，但恢复状态以真机后续观察为准，不得据此声称
  「自动休息已验收」。
