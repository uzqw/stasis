# 文档索引

本目录分为三类，命名与生命周期规则见 [../AGENTS.md](../AGENTS.md)「文档怎么写」。

## 长期文档（living spec）

随实现演进的规范与设计，改动需与代码同步。

| 文档 | 内容 |
| --- | --- |
| [design.md](design.md) | 跨平台行为契约、架构、故障策略、协议兼容、技术选型 |
| [implementation-plan.md](implementation-plan.md) | 分阶段实施与验收清单、回归用例 |

## 决策记录（ADR）

关键且不易回退的决策，一条一文件，见 [adr/](adr/)。

| 编号 | 决策 |
| --- | --- |
| [0001](adr/0001-rust-rewrite-shared-core.md) | 用 Rust 重写，并采用「共享核心 + 薄平台后端」结构 |

## 一次性记录

排查、分析、阶段性结论类文档，文件名带日期后缀，过期即归档。当前无。

## 规范入口

提交、分支与 AI 声明规范见 [../CONTRIBUTING.md](../CONTRIBUTING.md)；Agent 约定与禁区见
[../AGENTS.md](../AGENTS.md)。
