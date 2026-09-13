# Stasis 隔离测试环境 — virtme-ng

Date: 2026-09-14

## 选型

使用 [`virtme-ng`](https://github.com/arighi/virtme-ng) 运行基于宿主内核的轻量
VM，配合 `--disable-microvm` 获取标准 QEMU PC 机器类型的 `/dev/input/event*`
虚拟输入设备。已在宿主 Manjaro/Linux 7.1.13 验证可行。

| 方案 | 结果 |
| --- | --- |
| QEMU microVM (`virtme-ng` 默认) | 无 `/dev/input/event*` 设备，不满足 |
| `virtme-ng --disable-microvm` | 有 `event0..4`，可通过验证 |
| `systemd-nspawn` | 直接绑定宿主设备，有误抓宿主真实键盘风险，否决 |
| QEMU + Alpine ISO | 可行但需下载 ISO 与额外配置，磁盘/时间开销更大 |

## 环境要求

- 宿主 Linux，已安装 `virtme-ng`、`qemu-system-x86_64`、KVM 支持。
- 已加载 `uinput` 模块（宿主侧自动带入 VM 内核）。
- Stasis 在宿主编译后，二进制可直接在 VM 内执行（virtme-ng 挂载宿主文件系统）。

## 快速验证

```sh
# 1. 编译 Stasis（宿主侧）
cd /path/to/stasis
cargo build --example grab_test

# 2. 在 VM 内运行单元测试（验证基本行为）
virtme-ng --run --disable-microvm \
  --exec "cd /path/to/stasis && ./target/debug/deps/stasis-*"

# 3. 在 VM 内运行 grab 示例（验证 evdev 后端可启动）
virtme-ng --run --disable-microvm \
  --exec "cd /path/to/stasis && ./target/debug/examples/grab_test"
```

## 完整真机流程（供 leg-2 使用）

VM 内没有图形桌面时，Stasis 的 egui 窗口无法显示。若需要完整
锁—Caps×3—密码—解锁 流程验证，需以下任一方式：

1. **`virtme-ng --graphics`**：启动 QEMU 图形窗口，在 VM 桌面内运行 Stasis，
   并通过 VM 的虚拟键盘输入解锁手势。需注意：一旦 Stasis 抓取 evdev，
   QEMU 图形窗口的键盘事件也会被 grab，因此解锁必须通过同一键盘完成。
2. **命令文件解锁**：VM 内无图形时，跳过 UI，仅用 `input-locker-command.json`
   触发 lock/unlock，验证后端 grab/release 生命周期。
3. **VM 内 uinput 注入**：在 VM 内加载 `uinput` 并创建虚拟键盘，
   Stasis 抓取该虚拟设备，注入脚本发送解锁序列。
   需先执行 `modprobe uinput; mknod /dev/uinput c 10 223`。

## 限制

- VM 内无独立网络（`virtme-ng` 默认 `-net none`）， cargo 构建需在宿主完成。
- `uinput` 设备节点不会自动创建，首次使用需手动 `mknod`。
- 多键盘注入测试需要进一步确认 QEMU 虚拟设备枚举行为。

## 恢复手段

VM 可随时通过宿主 `kill` QEMU 进程强制终止，不会影响宿主输入。
