# gswitch

**NVIDIA Optimus 笔记本 GPU 切换工具** — 在集成显卡、NVIDIA 独立显卡、PRIME 混合模式与 VFIO 直通模式之间切换。

基于 [system76-power](https://github.com/pop-os/system76-power) 的图形切换逻辑，支持多个发行版的 initramfs 重建机制。

---

## 目录

- [支持的模式](#支持的模式)
- [依赖](#依赖)
- [安装](#安装)
- [卸载](#卸载)
- [使用指南](#使用指南)
  - [模式切换](#模式切换)
  - [查询与检测](#查询与检测)
  - [运行时电源管理](#运行时电源管理)
  - [GPU 缓存管理](#gpu-缓存管理)
  - [重置与恢复](#重置与恢复)
- [工作原理](#工作原理)
- [支持的发行版](#支持的发行版)
- [开发](#开发)
- [许可证](#许可证)

---

## 支持的模式

| 模式 | CLI 子命令 | 说明 | 适用场景 |
|------|-----------|------|---------|
| `integrated` | `gswitch integrated` | 仅集成显卡，将 NVIDIA 加入黑名单，udev 移除非显示设备 | 最大省电（不插电、开发者） |
| `hybrid` | `gswitch hybrid [--rtd3 <0-3>]` | PRIME 混合模式，按需卸载渲染，支持 RTD3 运行时电源管理 | 日常使用（兼顾续航与性能） |
| `nvidia` | `gswitch nvidia` | ⚠️ **伪独显模式**：通过 `__NV_PRIME_RENDER_OFFLOAD=1` 等环境变量强制应用渲染在 NVIDIA 显卡上，并非真正的独显切换。部分应用（如某些 Wayland 程序、Electron 应用）可能无法正确渲染。如有更好的强制渲染方案，请提交 Issue。 | 外接显示器、游戏、渲染（兼容性有限） |
| `passthrough` | `gswitch passthrough` | NVIDIA 绑定到 vfio-pci 驱动，直通给虚拟机 | 虚拟机 GPU 直通（VFIO） |

---

## 依赖

- **Rust** 1.85+（edition 2024）
- **NVIDIA 专有驱动**（nvidia-dkms / nvidia-driver）
- **systemd**
- **initramfs 工具之一**：dracut / update-initramfs / mkinitcpio

---

## 安装

### 编译与安装

```bash
cargo build --release
sudo install -m 755 target/release/gswitch /usr/local/bin/
```

### 验证

```bash
# 检查系统是否可切换
gswitch switchable

# 查看当前模式
gswitch query

# 查看版本与 commit（不传子命令）
gswitch --version
```

---

## 卸载

```bash
# 1. 恢复系统原始 GPU 配置（重置配置文件 + 重建 initramfs）
sudo gswitch reset

# 2. 删除二进制
sudo rm /usr/local/bin/gswitch

# 3. 清除缓存
sudo rm -rf /var/cache/gswitch
```

---

## 使用指南

> **注意：** 模式切换（integrated / hybrid / nvidia / passthrough）后需要**重启**才能完全生效。
>
> 运行时电源管理无需重启。

### 模式切换

所有模式切换命令需要 `sudo` 权限。

```bash
# 集成显卡模式 — 禁用 NVIDIA，最大省电
sudo gswitch integrated

# PRIME 混合模式 — 按需卸载渲染
sudo gswitch hybrid

# 混合模式 + RTD3 运行时电源管理（0=禁用，1-3=启用级别）
sudo gswitch hybrid --rtd3 2

# ⚠️ NVIDIA 渲染优先模式 — 通过环境变量强制渲染在 NVIDIA 显卡上
sudo gswitch nvidia

# VFIO 直通模式 — GPU 直通给虚拟机
sudo gswitch passthrough
```

### 查询与检测

以下命令**不需要** `sudo`：

```bash
# 查看当前 GPU 模式
gswitch query

# 检查系统是否支持 GPU 切换（DMI chassis + 是否存在双 GPU）
gswitch switchable

# 获取推荐的默认模式（基于机型 + RTD3 支持能力）
# 输出: integrated / hybrid / nvidia
gswitch default

# 检查外接显示器是否需要独立显卡（基于机型匹配）
gswitch ext-display

# 检查 NVIDIA GPU 是否支持运行时电源管理
gswitch runtime-pm
```

### 运行时电源管理

无需重启即可控制 NVIDIA GPU 电源：

```bash
# 查询 GPU 电源状态（输出: "开" / "关"）
gswitch power

# 开启 NVIDIA GPU（PCI rescan + power control）
sudo gswitch power on

# 关闭 NVIDIA GPU（解绑所有功能号 + 移除 PCI 设备）
sudo gswitch power off

# 自动配置电源（基于当前模式 + RTD3 支持能力）
sudo gswitch power auto
```

**关闭流程说明：**

1. 检查 NVIDIA GPU 上是否有运行中的进程（通过 `nvidia-smi`），有则拒绝关闭
2. 获取 NVIDIA PCI 地址，找出同一 slot 上的所有功能号（VGA、音频、USB、UCSI）
3. **按功能号降序**逐个解绑（子设备优先于主设备），失败则尝试重新绑定恢复
4. **按功能号降序**逐个移除设备，失败则尝试 PCI rescan 恢复

### GPU 缓存管理

gswitch 将 NVIDIA GPU 的 PCI 地址持久化到 `/var/cache/gswitch/gpu-cache.json`，
以便在集成模式下（NVIDIA 已被 udev 移除后）仍能定位 GPU。

```bash
# 创建/更新缓存（需要 hybrid 或 passthrough 模式）
sudo gswitch cache-create

# 查询缓存内容
gswitch cache-query

# 删除缓存
sudo gswitch cache-delete
```

缓存在 `integrated` 切换时自动更新（若 sysfs 中有 NVIDIA 设备）；

从 `passthrough` 切换到其他模式时，缓存用于恢复 PCI 地址。

### 重置与恢复

```bash
# 恢复出厂设置：删除所有 gswitch 配置文件 + 重建 initramfs
sudo gswitch reset
```

重置操作会清理以下文件：

| 文件 | 路径 |
|------|------|
| GPU modprobe 配置 | `/etc/modprobe.d/gswitch-gpu.conf` |
| NVIDIA modeset 配置 | `/etc/modprobe.d/gswitch-nvidia-modeset.conf` |
| 集成模式 udev 规则 | `/etc/udev/rules.d/50-gswitch-remove-nvidia.rules` |
| 运行时 PM udev 规则 | `/etc/udev/rules.d/80-gswitch-nvidia-pm.rules` |
| PRIME 标志 | `/etc/prime-discrete` |
| NVIDIA 环境变量 | `/etc/environment.d/gswitch-nvidia.conf` |
| X11 NVIDIA 配置 | `/etc/X11/xorg.conf.d/11-nvidia-discrete.conf` |
| dracut VFIO 配置 | `/etc/dracut.conf.d/gswitch-vfio.conf` |
| mkinitcpio VFIO 配置 | `/etc/mkinitcpio.conf.d/gswitch-vfio.conf` |
| initramfs-tools modules | `/etc/initramfs-tools/modules`（仅移除 vfio_pci 行） |

---

## 工作原理

gswitch 通过修改系统配置文件 + 重建 initramfs 实现 GPU 模式切换。

> **关于 nvidia 模式的说明：** 该模式并非真正的"独显切换"（即完全由 NVIDIA 显卡接管显示输出），而是通过设置 `__NV_PRIME_RENDER_OFFLOAD=1`、`DRI_PRIME=1` 等环境变量，配合 X11 PrimaryGPU 配置，**请求**应用在 NVIDIA 显卡上渲染。
> - 并非所有应用都遵循这些环境变量
> - Wayland 环境下部分应用不受 `__NV_PRIME_RENDER_OFFLOAD` 控制
> - Electron/Chromium 类应用可能存在渲染异常
> - 真正的独显模式需要 BIOS MUX 硬件支持或 NVIDIA 驱动的 Dynamic Boost 机制
>
> 如果你有其他方法能在不使用环境变量的情况下强制应用渲染在 NVIDIA 显卡上，欢迎提交 Issue。

核心流程：

```
用户请求切换
     │
     ▼
 检测可切换性 ──→ DMI chassis type 是否为笔记本？
                   sysfs 中是否存在双 GPU？
                   缓存是否存在？
     │
     ▼
 保存配置快照 ──→ 记录所有受影响文件的当前内容 + systemd 服务状态
     │
     ▼
 清理旧配置 ──→ 移除所有 gswitch 生成的文件
     │
     ▼
 写入新配置 ──→ 根据目标模式写入对应的 modprobe / udev / X11 / 环境变量配置
     │
     ▼
 追加挂起管理 ──→ 检测 S0ix / S3 休眠模式，追加 NVreg_PreserveVideoMemoryAllocations
     │
     ▼
 重建 initramfs ──→ 根据发行版调用 dracut / update-initramfs / mkinitcpio
     │
     ▼
 成功 ✓                     失败 ✗
                      ──→ 回滚配置快照
                          重建 initramfs → 提示重启
```

### 每种模式的操作细节

| 模式 | modprobe | udev | 其他 |
|------|----------|------|------|
| **Integrated** | 黑名单 `nvidia` / `nouveau`，install 拦截 | 移除 NVIDIA 设备（音频、USB、VGA） | 删除 modeset 配置 |
| **Hybrid** | 空（允许所有驱动） | 运行时 PM 规则（bind → auto, unbind → on） | 写入 modeset 配置（含可选 RTD3） |
| **Nvidia** | 空 | — | 写入 modeset、X11 PrimaryGPU、环境变量 `__NV_PRIME_RENDER_OFFLOAD=1` 等、启用 nvidia-fallback |
| **Passthrough** | 黑名单 + vfio-pci ids= 绑定 | — | 添加 vfio_pci 到 initramfs 模块 |

### 原子性与回滚

- **原子写入**：所有文件通过临时文件 + `rename()` 写入，避免写入中断导致残缺文件
- **配置快照**：切换前保存所有受影响文件的完整内容及 systemd 服务启用状态
- **自动回滚**：任何阶段的失败触发快照恢复 + initramfs 重建，使系统回到切换前状态

---

## 支持的发行版

| 发行版 | initramfs 工具 | 路径覆盖 |
|--------|---------------|---------|
| Fedora / RHEL / openSUSE | `dracut --force --regenerate-all` | 全部 |
| Fedora Silverblue / Kinoite | `rpm-ostree initramfs --enable --arg=--force` | 全部 |
| Debian / Ubuntu | `update-initramfs -u` | 全部 |
| Arch Linux | `mkinitcpio -P` | 全部 |

自动检测机制（按优先级）：

1. OSTree 系统 → `rpm-ostree`
2. `/usr/bin/dracut` 存在 → `dracut`
3. `/usr/sbin/update-initramfs` 存在 → `update-initramfs`
4. `/usr/bin/mkinitcpio` 存在 → `mkinitcpio`

---

## 开发

### 项目结构

```
src/
├── main.rs         # 入口：初始化日志 + CLI 解析 + 调度
├── lib.rs          # 模块声明
├── cli.rs          # clap CLI 参数定义 + 子命令路由
├── config.rs       # 路径常量、路径注入（测试用）、dry-run 标志
├── detector.rs     # sysfs 硬件检测、GPU 设备发现、休眠模式检测
├── graphics.rs     # 模式切换控制器、PCI ID 解析、modeset 生成
├── helper.rs       # 文件 I/O、initramfs 重建、服务管理、运行时电源控制
├── cache.rs        # GPU PCI 缓存持久化（JSON）
└── error.rs        # 错误类型定义（thiserror）
tests/
└── integration_tests.rs  # 集成测试（路径注入 + 临时根目录）
```

### 测试

所有测试使用 **thread-local 路径注入**（`config::set_root_for_test()`），天然支持并行执行。

```bash
# 运行所有测试
cargo test

# 仅运行单元测试
cargo test --lib

# 仅运行集成测试
cargo test --test integration_tests
```

测试特性：

- 文件 I/O 操作通过路径重定向到临时目录，不触及真机系统
- systemd 交互在测试模式下自动跳过（返回 `None` / `Ok(())`）
- dry-run 模式验证：文件不应被实际创建或删除
- 配置快照的保存/回滚完整性验证

### 构建说明

```bash
# 调试构建
cargo build

# 发布构建
cargo build --release

# 构建时自动注入 git commit hash（build.rs）
# 若不在 git 仓库中，则 fallback 为 "unknown"
```

---

## 许可证

GPL-3.0