# Stasis

Stasis Domain

**Pause the machine. Rest the human.**

Input Locker 的 Rust 重写项目：暂停键盘、鼠标输入，让人休息。

目标平台：Windows、Linux（X11 / Wayland）、macOS。核心原则：**行为定义一份，平台只适配输入与系统
资源；
解锁不依赖窗口焦点或 UI 响应。**

## 当前状态

设计阶段，尚无 Rust 实现或可用安装包。平台支持是目标，不是已验证的能力声明。

- [设计方案](docs/design.md)：范围、跨平台行为、线程与资源模型、协议兼容、安全边界。
- [实施与验收计划](docs/implementation-plan.md)：按可验证成果推进，不按代码量判定完成。
- [决策记录](docs/adr/0001-rust-rewrite-shared-core.md)：为什么重写、为什么是共享核心结构。

新项目独立开发，原 `../input-locker` 保留使用。切换前不得让两个程序同时消费同一事件目录或抓取输
入。

## 规范

- 提交信息、分支命名、AI 声明：[CONTRIBUTING.md](CONTRIBUTING.md)（提交信息统一英文）。
- Agent 约定、文档规则、禁区：[AGENTS.md](AGENTS.md)。
- 文档索引与分类：[docs/README.md](docs/README.md)。

克隆或新建 worktree 后先安装钩子（`core.hooksPath` 是本地配置，不随 clone 传播）：

```sh
git config core.hooksPath .githooks
```
