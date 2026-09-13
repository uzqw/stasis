# 项目契约（Agent 约定）

Stasis：**Pause the machine. Rest the human.** —— 暂停键盘鼠标输入，让人休息的三端工具。

## 项目概述

Rust 重写项目，设计与实施计划见 [docs/design.md](docs/design.md)、
[docs/implementation-plan.md](docs/implementation-plan.md)。当前处于实现前阶段：仓库内只有文档、
守卫工具和钩子，尚无 Cargo package 与可运行程序。

原 Python 版 `../input-locker` 保留使用，不在本仓库内改动。

## 仓库地图

| 路径 | 内容 |
| --- | --- |
| `docs/` | 设计、实施计划、ADR；索引见 [docs/README.md](docs/README.md) |
| `docs/adr/` | 关键决策记录（架构、协议、平台取舍） |
| `tools/guard/` | 仓库守卫（文档、行宽、凭据），零依赖 Rust 二进制 |
| `.githooks/` | `pre-commit`、`commit-msg`、`commit-lint.txt` 动词表 |
| `CONTRIBUTING.md` | 提交、分支、AI 声明规范 |
| `README.md` | 对外入口与当前状态 |

## 文档怎么写

- **分类**：长期有效的规范/设计放 `docs/` 顶层，随实现演进；一次性分析、排查记录带日期后缀
  （如 `note-2026-09-13.md`），过期即归档。关键决策另记入 `docs/adr/`。
- **结构**：标题从 `#` 起逐级递增不跳级；长文档开头给结论或目录；仓库内引用一律相对路径。
- **行宽**：正文行视觉宽度 ≤100（CJK/全角按 2 列计）；表格行、代码块、单个长 URL 豁免。
  中文按此规则约每行 50 个汉字，需要主动换行，不要写成整段一行。
- **表格**：只用于小表（≤4 列、单元格短）；复杂内容用列表或拆表。
- **代码块**：必须标注语言（```text、```rust、```sh、```json）；命令必须真实可执行，禁止编造
  不存在的路径或 flag。
- **事实性**：写「为什么」和稳定契约，不写会过期的细节；代码或行为变更时同步更新相关文档。
- **检查**：`.githooks/pre-commit` 对暂存 `.md` 做文档守卫；手动跑
  `cargo run --quiet --manifest-path tools/guard/Cargo.toml`。

## 提交规范

- 提交信息**统一英文**，type 与动词表见 [CONTRIBUTING.md](CONTRIBUTING.md)；机器强制由
  `.githooks/commit-msg` 完成，动词表单源在 `.githooks/commit-lint.txt`。
- 钩子经 `core.hooksPath` 挂载，**不随 clone 传播**，新 clone / 新 worktree 先装一次：

```sh
git config core.hooksPath .githooks
```

## 平台安全（重要）

本项目操作的是用户真实输入设备，写错会把人锁在机器外：

- 真实输入抓取、系统锁屏、USB 策略写入类测试**只在显式授权的桌面执行**，并先准备恢复通道
  （第二台设备、SSH、可强制终止的终端）。
- 自动化测试默认使用假后端与临时目录，不得抓取开发者真实键鼠或改系统配置。
- 抓取失败、部分成功必须回滚并如实报错，不允许「部分锁定却报告成功」。
- 三端行为差异（Wayland 无全局输入权限、macOS 辅助功能授权、Windows 钩子超时）要如实记录，
  不用「编译通过」代替真机验证。

## Guardrails（禁区）

- 不提交 `.env*`、密钥、令牌、证书私钥或真实用户数据；配置只写变量名不写值。
- 不提交构建产物与本地状态（`target/`、`.pi-web/`、运行期事件目录、日志）。
- 不在文档或代码里写开发者机器的绝对路径；仓库内引用一律相对路径。
- 不擅自更改 aide 线协议字段名与文件名（`input-locker-events/`、`input-locker-command.json`
  等）；协议变更需先更新 [docs/design.md](docs/design.md) 与 aide 侧。
- 不让两个消费者共用同一个事件目录（Stasis 与旧 Python 程序不得同时运行在同一目录上）。
- 不新增生产依赖而不说明理由；不引入与本项目规模不匹配的框架。

## 提交前测试

1. 守卫（必跑，快）：`cargo run --quiet --manifest-path tools/guard/Cargo.toml`
2. 格式化与静态检查：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`
3. 单元测试：`cargo test`
4. 守卫自身的测试：`cargo test --manifest-path tools/guard/Cargo.toml`
5. 真实桌面验证按 [docs/implementation-plan.md](docs/implementation-plan.md) 的分阶段出口执行。

缺依赖或环境跑不了的检查，明确列出未验证项、原因和风险，**不得声称验证通过**。
