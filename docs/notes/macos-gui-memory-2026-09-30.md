# macOS GUI 内存优化实测（2026-09-30）

结论：两个常驻 GUI 的合计 physical footprint 从约 316 MiB 降至约 57–60 MiB，
降低约 81%。保留中文、原字号、窗口尺寸、透明置顶及刷新频率；Dock 改用系统默认图标。
本记录不代表 macOS 真实输入锁定/解锁验收通过。

## 测量口径

- 同一台 Apple Silicon Mac，Darwin 25.5.0，release 构建，由原 launchd agent 启动。
- 基线代码：`9e2a361`；不修改其他常驻服务，不以 RSS 或压缩后的驻留量替代 footprint。
- 使用 `footprint -p <pid>`，结合 `vmmap -summary <pid>` 分类核对。
- 最终部署重启后，先测启动约 10 秒，再待稳定后每 30 秒采样，共 120 秒、4 个样本。
- stasis 普通未锁定窗口，rest-break-ui 常规摘要窗口；窗口可见状态、字形缓存和交互会
  影响数字，单轮实验数字不是所有运行状态的上限。

## 结果

以下数值均为 MiB，四舍五入；中间轮次用于区分收益，不作为最终稳态承诺。

| 阶段 | stasis | rest-break-ui |
| --- | --- | --- |
| 原运行进程 | 168 | 148 |
| 原版本重新启动约 10 秒 | 168 | 147 |
| 仅字体映射，保留 glow 和图标 | 65 | 39–41 |
| 再切换 wgpu/Metal，保留图标 | 34 | 34 |
| 最终版本启动约 10 秒 | 38 | 24 |
| 最终版本稳定后连续采样 | 33.1 | 24.0–26.3 |
| 最终进程已记录的 footprint 峰值 | 41.9 | 27.1 |

中间轮次的 stasis Metal 窗口 IOSurface 很少；最终采样为约 7.9 MiB，
因此不能把中间图标实验的总量差直接当作 stasis 图标的净收益。
最终数字来自恢复正式源码和无截图插桩二进制后的新进程。

## 根因与改动

### CJK 字体占了两个完整堆副本

Mac 没有首选的 PingFang 路径，实际命中 `STHeiti Light.ttc`，大小约 53.2 MiB。
不是候选表末尾约 22 MiB 的 Hiragino。`std::fs::read` 持有整个 TTC，
egui 0.36.2 的 `blob_from_font_data` 又克隆 `FontData` 中的 `Cow::Owned`。
两个进程各自都有两块约 53.2 MiB 的 `MALLOC_LARGE`，即约 106 MiB 私有字体数据。

两个 GUI 现在复用 [字体模块](../../src/cjk_font.rs)：

- macOS 用 `memmap2` 只读映射系统字体，以 `OnceLock` 保持映射存活，交给
  `FontData::from_static`；克隆只复制借用，不复制字体字节。
- 新增直接依赖 `memmap2` 的理由是安全封装文件映射，避免自写 mmap FFI。
  它原已存在于锁文件，只在 macOS GUI 启用；不能映射用户可写字体。
- 仍加载原完整字库、原 face，不改字体候选顺序，不裁剪动态中文和路径字符。
- `vmmap` 核对：字体为只读文件映射，约 53.2 MiB 虚拟范围，仅少量页面被访问，
  没有两块 53.2 MiB 私有堆分配；不能把该虚拟范围误报为 footprint。

### 使用原生 Metal，去掉无关图标

- 仅 macOS 启用 eframe 的 wgpu 后端，使用原生 Metal；Linux/Windows 保留 glow。
  这是已有 GUI 依赖的后端特性，新增传递依赖换取实测内存收益。
- macOS 显式使用空 `IconData`，跳过默认 egui 图标的解码及 AppKit 图像保留。
  保留图标的 Metal 轮次各有约 8 MiB `CG image`，最终版本没有该项。
- 没有缩小窗口、关闭透明、降低刷新频率或移除置顶。原 rest-break-ui 的
  IOSurface 只有约 0.6–0.7 MiB，并不是 100 MiB 级别大头。
- 不采用所谓软件渲染捷径：glutin 的 CGL 实现只有请求硬件加速时添加相应属性，
  关闭该请求不等于强制软件渲染，也不能保证降低 footprint。

## 验证与边界

- Linux 与 Mac 均通过两个 crate 的 `clippy --all-targets -- -D warnings` 和测试。
  Linux 主 crate 52 项；Mac 主 crate 52 项，含 macOS 字体回归测试。
  rest-break 核心、集成测试 57 项，Mac UI 另有 1 项字体回归测试。
- 字体回归验证重复加载及 `FontData::clone` 保持同一借用指针，并检查比例与等宽
  两种字体族均能显示代表性中文标签。
- Mac 使用临时 `ViewportCommand::Screenshot` 插桩读取实际 Metal 渲染结果，
  人工检查 stasis 中文按钮与 rest-break-ui 展开后的全部明细；未抓取整个桌面。
  插桩、截图和运行日志均不入库，部署时已恢复正式版本。
- 窗口显示正常不等于真实输入锁定通过。本轮未触发真实抓取、修改密码或系统策略；
  不声称已验证 macOS 辅助功能授权与真实解锁。
- 未做 Windows 真机回归；渲染后端和字体读取策略未改，退出回调仍保留 glow 签名。
- Linux 格式检查、仓库守卫及守卫自身测试通过；rest-break 无 UI 特性的测试通过。
  Mac 核对默认 rest-break 依赖树不含 eframe、egui、wgpu 或 memmap2。

## 复测命令

在部署该仓库的 Mac 执行，不会触发输入锁定：

```sh
footprint -p "$(pgrep -x stasis)"
footprint -p "$(pgrep -x rest-break-ui)"
vmmap -summary "$(pgrep -x stasis)"
vmmap -summary "$(pgrep -x rest-break-ui)"
footprint -p "$(pgrep -x stasis)" -p "$(pgrep -x rest-break-ui)" \
  --noCategories -f bytes --sample 30 --sample-duration 120
```
