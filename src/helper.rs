//! 文件 I/O 与系统命令辅助函数

use crate::error::GswitchError;
use crate::graphics::GraphicsMode;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};

/// 临时文件序号计数器，避免同进程内的命名碰撞
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 原子写入文件：先写入临时文件再重命名
fn atomic_write(path: &str, content: &[u8], executable: bool) -> Result<(), GswitchError> {
    if crate::config::is_dry_run() {
        info!(
            "[dry-run] 将写入文件: {} ({} bytes, executable={})",
            path,
            content.len(),
            executable
        );
        return Ok(());
    }

    let resolved = crate::config::resolve_path(path);
    if let Some(parent) = Path::new(&resolved).parent() {
        fs::create_dir_all(parent)
            .map_err(|e| GswitchError::Gpu(format!("create dir {:?} failed: {}", parent, e)))?;
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let cnt = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = format!("{}.tmp.{}-{}-{}", resolved, std::process::id(), ts, cnt);

    fs::write(&tmp, content)
        .map_err(|e| GswitchError::Gpu(format!("write tmp file {} failed: {}", tmp, e)))?;

    if executable {
        let meta = fs::metadata(&tmp)
            .map_err(|e| GswitchError::Gpu(format!("metadata failed: {}", e)))?;
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp, perms)
            .map_err(|e| GswitchError::Gpu(format!("set permissions failed: {}", e)))?;
    }

    fs::rename(&tmp, &resolved)
        .map_err(|e| GswitchError::Gpu(format!("rename to {} failed: {}", resolved, e)))?;

    debug!("文件已写入: {}", resolved);
    Ok(())
}

/// 原子写入文本文件
pub fn write_file(path: &str, content: &str) -> Result<(), GswitchError> {
    atomic_write(path, content.as_bytes(), false)
}

/// 删除文件，忽略 NotFound 错误
pub fn remove_file(path: &str) -> Result<(), GswitchError> {
    if crate::config::is_dry_run() {
        info!("[dry-run] 将删除文件: {}", path);
        return Ok(());
    }
    let resolved = crate::config::resolve_path(path);
    match fs::remove_file(&resolved) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(GswitchError::Gpu(format!(
            "删除 {} 失败: {}",
            resolved, e
        ))),
    }
}

/// 检查文件是否存在（尊重 dry-run 和路径注入）
pub fn file_exists(path: &str) -> bool {
    Path::new(&crate::config::resolve_path(path)).exists()
}

/// 读取文件内容（尊重路径注入）
pub fn read_file(path: &str) -> Result<String, GswitchError> {
    let resolved = crate::config::resolve_path(path);
    fs::read_to_string(&resolved)
        .map_err(|e| GswitchError::Gpu(format!("读取 {} 失败: {}", resolved, e)))
}

/// 清理所有 gswitch 生成的配置文件
pub fn cleanup() -> Result<(), GswitchError> {
    info!("正在清理 gswitch 配置文件...");
    use crate::config::*;
    let mut errors: Vec<String> = Vec::new();

    for path in [
        MODPROBE_GPU_PATH,
        MODESET_PATH,
        UDEV_INTEGRATED_PATH,
        UDEV_PM_PATH,
        PRIME_DISCRETE_PATH,
        NV_ENV_PATH,
        XORG_CONF_NVIDIA_PATH,
        XORG_CONF_NVIDIA_FALLBACK_PATH,
        DRACUT_VFIO_CONF_PATH,
        MKINITCPIO_VFIO_CONF_PATH,
    ] {
        if let Err(e) = remove_file(path) {
            errors.push(e.to_string());
        }
    }
    if let Err(e) = remove_vfio_from_initramfs_modules() {
        errors.push(e.to_string());
    }

    if !errors.is_empty() {
        return Err(GswitchError::Gpu(format!(
            "清理部分失败: {}",
            errors.join("; ")
        )));
    }
    Ok(())
}

/// 从 initramfs-tools/modules 文件中移除 vfio_pci 相关行
fn remove_vfio_from_initramfs_modules() -> Result<(), GswitchError> {
    use crate::config::INITRAMFS_TOOLS_MODULES_PATH;
    let path = INITRAMFS_TOOLS_MODULES_PATH;
    if !file_exists(path) {
        return Ok(());
    }
    let content = read_file(path)?;
    let filtered: Vec<&str> = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed != "vfio_pci" && !trimmed.starts_with("# Added by gswitch")
        })
        .collect();
    let new_content = format!("{}\n", filtered.join("\n"));
    if new_content != content {
        write_file(path, &new_content)?;
    }
    Ok(())
}

/// 将 vfio-pci 模块添加到 initramfs（确保 VFIO 驱动先于 nvidia 加载）
pub fn add_vfio_to_initramfs() -> Result<(), GswitchError> {
    use crate::config::*;
    info!("正在将 vfio-pci 添加到 initramfs...");

    // dracut (Fedora/RHEL/openSUSE)
    if Path::new("/usr/bin/dracut").exists() {
        let conf_dir = crate::config::resolve_path("/etc/dracut.conf.d");
        fs::create_dir_all(&conf_dir)
            .map_err(|e| GswitchError::Gpu(format!("创建目录 {} 失败: {}", conf_dir, e)))?;
        write_file(
            DRACUT_VFIO_CONF_PATH,
            "# Automatically generated by gswitch\nadd_drivers+=\" vfio_pci vfio vfio_iommu_type1 \"",
        )?;
        info!("已写入 dracut VFIO 配置: {}", DRACUT_VFIO_CONF_PATH);
        return Ok(());
    }

    // update-initramfs (Debian/Ubuntu)
    if Path::new("/usr/sbin/update-initramfs").exists()
        || Path::new("/sbin/update-initramfs").exists()
    {
        let path = INITRAMFS_TOOLS_MODULES_PATH;
        let existing = read_file(path).unwrap_or_default();
        if !existing.contains("vfio_pci") {
            let mut new_content = existing.trim_end().to_string();
            if !new_content.is_empty() {
                new_content.push('\n');
            }
            new_content.push_str("# Added by gswitch for VFIO passthrough\nvfio_pci\n");
            write_file(path, &new_content)?;
            info!("已将 vfio_pci 添加到 {}", path);
        } else {
            debug!("vfio_pci 已存在于 {}", path);
        }
        return Ok(());
    }

    // mkinitcpio (Arch Linux, v37+ 支持 /etc/mkinitcpio.conf.d/)
    if Path::new("/usr/bin/mkinitcpio").exists() {
        let conf_dir = crate::config::resolve_path("/etc/mkinitcpio.conf.d");
        fs::create_dir_all(&conf_dir)
            .map_err(|e| GswitchError::Gpu(format!("创建目录 {} 失败: {}", conf_dir, e)))?;
        write_file(
            MKINITCPIO_VFIO_CONF_PATH,
            "# Automatically generated by gswitch\nMODULES=(vfio_pci vfio vfio_iommu_type1 vfio_virqfd)",
        )?;
        info!(
            "已写入 mkinitcpio VFIO drop-in 配置: {}",
            MKINITCPIO_VFIO_CONF_PATH
        );
        return Ok(());
    }

    warn!("无法自动添加 vfio_pci 到 initramfs，请手动配置以确保 VFIO 驱动先于 nvidia 驱动加载");
    Ok(())
}

/// 重建 initramfs（支持 dracut、update-initramfs 和 ostree）
pub fn rebuild_initramfs() -> Result<(), GswitchError> {
    if crate::config::is_dry_run() || crate::config::is_test_mode() {
        info!("[dry-run] 将重建 initramfs");
        return Ok(());
    }

    info!("正在重建 initramfs...");

    // OSTree 系统（Fedora Silverblue 等）
    if Path::new("/ostree").exists() || Path::new("/sysroot/ostree").exists() {
        info!("检测到 OSTree，使用 rpm-ostree");
        let status = run_status("rpm-ostree", ["initramfs", "--enable", "--arg=--force"])?;
        return ensure_success(status);
    }

    // Debian/Ubuntu
    if Path::new("/usr/sbin/update-initramfs").exists()
        || Path::new("/sbin/update-initramfs").exists()
    {
        info!("检测到 update-initramfs");
        let status = run_status("update-initramfs", ["-u"])?;
        return ensure_success(status);
    }

    // Arch Linux (mkinitcpio)
    if Path::new("/usr/bin/mkinitcpio").exists() {
        info!("检测到 mkinitcpio (Arch Linux)");
        let status = run_status("mkinitcpio", ["-P"])?;
        return ensure_success(status);
    }

    // Fallback: dracut
    if Path::new("/usr/bin/dracut").exists() || Path::new("/usr/sbin/dracut").exists() {
        if Path::new("/usr/bin/systemd-inhibit").exists() {
            let status = run_status(
                "systemd-inhibit",
                [
                    "--who=gswitch",
                    "--why=Rebuilding initramfs",
                    "--",
                    "dracut",
                    "--force",
                    "--regenerate-all",
                ],
            )?;
            ensure_success(status)
        } else {
            let status = run_status("dracut", ["--force", "--regenerate-all"])?;
            ensure_success(status)
        }
    } else {
        Err(GswitchError::Gpu("未找到可用的 initramfs 重建工具（支持: dracut, update-initramfs, mkinitcpio, rpm-ostree）".into()))
    }
}

/// 获取 X11 配置目录路径
pub fn get_xorg_conf_path() -> &'static str {
    if file_exists("/etc/X11/xorg.conf.d") {
        crate::config::XORG_CONF_NVIDIA_PATH
    } else {
        crate::config::XORG_CONF_NVIDIA_FALLBACK_PATH
    }
}

/// 写入 X11 PrimaryGPU 配置（NVIDIA 独立显卡模式）
pub fn write_xorg_nvidia_config() -> Result<(), GswitchError> {
    let path = get_xorg_conf_path();
    info!("正在写入 X11 配置: {}", path);
    write_file(path, crate::config::XORG_CONF_NVIDIA_CONTENT)
}

/// 写入 NVIDIA 环境变量配置（独立显卡模式）
pub fn write_nvidia_env_config() -> Result<(), GswitchError> {
    info!("正在写入 NVIDIA 环境变量配置");
    write_file(
        crate::config::NV_ENV_PATH,
        crate::config::NV_ENV_CONTENT,
    )
}

/// 启用或禁用 systemd 服务
pub fn toggle_service(name: &str, enable: bool) -> Result<(), GswitchError> {
    let action = if enable { "enable" } else { "disable" };
    if crate::config::is_dry_run() || crate::config::is_test_mode() {
        info!("[dry-run] systemctl {} {}", action, name);
        return Ok(());
    }
    let status = run_status("systemctl", [action, name])?;
    if status.success() {
        debug!("service toggled: {} {}", action, name);
        Ok(())
    } else {
        Err(GswitchError::Process(format!(
            "systemctl {} {} 失败 (退出码: {:?})",
            action,
            name,
            status.code()
        )))
    }
}

/// 查询 systemd 服务是否已启用
pub fn service_is_enabled(name: &str) -> Option<bool> {
    if crate::config::is_dry_run() || crate::config::is_test_mode() {
        return None;
    }
    Command::new("systemctl")
        .args(["is-enabled", name])
        .output()
        .ok()
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            stdout == "enabled" || stdout == "static"
        })
}

/// 启用或禁用 NVIDIA 挂起/休眠服务（容错处理）
pub fn configure_nvidia_suspend_services(enable: bool) {
    for svc in crate::config::NVIDIA_SUSPEND_SERVICES {
        if let Err(e) = toggle_service(svc, enable) {
            warn!("NVIDIA 挂起服务切换失败: {} - {}", svc, e);
        }
    }
}

/// 写入 PRIME 独立显卡模式标志文件
pub fn set_prime_discrete(mode: &str) -> Result<(), GswitchError> {
    info!("正在设置 PRIME 独立显卡模式: {}", mode.trim());
    write_file(crate::config::PRIME_DISCRETE_PATH, mode)
}

/// 在 modprobe 文件中追加挂起电源管理配置
pub fn append_sleep_config(mode: GraphicsMode) -> Result<(), GswitchError> {
    if mode == GraphicsMode::Integrated || mode == GraphicsMode::Passthrough {
        return Ok(());
    }

    let sleep_mode = crate::detector::Detector::detect_sleep_mode();
    let (sleep_content, key_line) = match sleep_mode {
        crate::detector::SleepMode::S0ix => (
            crate::config::MODPROBE_S0IX,
            "NVreg_EnableS0ixPowerManagement=1",
        ),
        crate::detector::SleepMode::S3 => (
            crate::config::MODPROBE_S3,
            "NVreg_PreserveVideoMemoryAllocations=1",
        ),
        crate::detector::SleepMode::Unknown => {
            warn!("无法检测系统休眠模式，跳过挂起配置");
            return Ok(());
        }
    };

    let path = crate::config::MODPROBE_GPU_PATH;
    let existing = read_file(path).unwrap_or_default();

    if existing.contains(key_line) {
        debug!("挂起配置已存在，跳过");
        return Ok(());
    }

    let merged = if existing.trim().is_empty()
        || existing.trim_end() == "# Automatically generated by gswitch"
    {
        format!("{}\n", sleep_content.trim())
    } else {
        format!("{}\n{}", existing.trim_end(), sleep_content.trim())
    };

    write_file(path, &merged)?;
    configure_nvidia_suspend_services(true);
    Ok(())
}

// ====== 命令执行 ======

/// 运行命令并返回 ExitStatus
pub fn run_status<I, S>(cmd: &str, args: I) -> Result<ExitStatus, GswitchError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(cmd)
        .args(args)
        .status()
        .map_err(|e| GswitchError::Process(format!("执行 {} 失败: {}", cmd, e)))
}

/// 检查退出状态，失败则返回错误
pub fn ensure_success(status: ExitStatus) -> Result<(), GswitchError> {
    if status.success() {
        Ok(())
    } else {
        Err(GswitchError::Process(format!(
            "命令失败，退出码: {:?}",
            status.code()
        )))
    }
}

// ====== 运行时电源控制 ======

/// 运行时开启 NVIDIA GPU（无需重启）
pub fn runtime_power_on() -> Result<(), GswitchError> {
    if crate::config::is_dry_run() {
        info!("[dry-run] 将开启 NVIDIA GPU (PCI rescan + power control)");
        return Ok(());
    }

    info!("正在开启 NVIDIA GPU...");

    fs::write("/sys/bus/pci/rescan", "1")
        .map_err(|e| GswitchError::Gpu(format!("PCI rescan failed: {}", e)))?;

    if let Ok(pci_id) = crate::detector::Detector::get_nvidia_pci_id() {
        let mode = crate::detector::Detector::query_current_mode();
        apply_power_control(&pci_id, mode)?;
    }

    Ok(())
}

/// 通过 PATH 环境变量查找 nvidia-smi
fn find_nvidia_smi() -> Option<String> {
    // 遍历 PATH 环境变量
    if let Ok(paths) = std::env::var("PATH") {
        for dir in paths.split(':') {
            let full = Path::new(dir).join("nvidia-smi");
            if full.exists() {
                return Some(full.to_string_lossy().to_string());
            }
        }
    }
    // 备选 ABSOLUTE 路径（容器 / 非标准环境）
    for p in &[
        "/usr/bin/nvidia-smi",
        "/usr/local/bin/nvidia-smi",
        "/usr/local/sbin/nvidia-smi",
    ] {
        if Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    None
}

/// 检查是否有进程正在使用 NVIDIA GPU
fn has_nvidia_processes() -> bool {
    let smi = match find_nvidia_smi() {
        Some(p) => p,
        None => return false,
    };

    let check = |args: &[&str]| -> bool {
        Command::new(&smi)
            .args(args)
            .output()
            .ok()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| !l.trim().is_empty())
            })
            .unwrap_or(false)
    };

    check(&[
        "--query-compute-apps=pid,process_name",
        "--format=csv,noheader",
    ]) || check(&[
        "--query-graphics-apps=pid,process_name",
        "--format=csv,noheader",
    ])
}

/// 运行时关闭 NVIDIA GPU（无需重启）
pub fn runtime_power_off() -> Result<(), GswitchError> {
    if crate::config::is_dry_run() {
        info!("[dry-run] 将关闭 NVIDIA GPU (解绑 + 移除)");
        return Ok(());
    }

    info!("正在关闭 NVIDIA GPU...");

    if has_nvidia_processes() {
        return Err(GswitchError::Gpu(
            "NVIDIA GPU 上仍有进程在运行，请先终止它们".into(),
        ));
    }

    let pci_id = match crate::detector::Detector::get_nvidia_pci_id() {
        Ok(id) => id,
        Err(_) => {
            // GPU 已不在 sysfs 中（可能已被 udev 移除或从未在线），
            // 视为已关闭，幂等成功
            if !crate::detector::Detector::is_nvidia_online() {
                info!("NVIDIA GPU 已处于关闭状态");
                return Ok(());
            }
            // GPU 在线但无法获取 PCI ID，属于异常
            return Err(GswitchError::Gpu(
                "无法获取 NVIDIA GPU 的 PCI ID，请检查系统状态".into(),
            ));
        }
    };

    // 查找同一 PCI slot 上的所有功能号（音频、USB 等）。
    // 绝大多数 Optimus 笔记本的 NVIDIA 设备（VGA/音频/USB/UCSI）共享同一 slot，
    // 因此按 slot 遍历即可覆盖所有需要解绑的设备。
    let pci_path = Path::new("/sys/bus/pci/devices");
    let slot = pci_id.split('.').next().unwrap_or("");
    let entries = fs::read_dir(pci_path)
        .map_err(|e| GswitchError::Gpu(format!("读取 PCI 设备失败: {}", e)))?;

    let mut functions: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.split('.').next().unwrap_or("") == slot {
            let vendor_path = pci_path.join(&name).join("vendor");
            if let Ok(vendor) = fs::read_to_string(&vendor_path)
                && vendor.trim() == "0x10de"
            {
                functions.push(name);
            }
        }
    }

    // 按功能号降序排列：先解绑子设备（高功能号）再解绑父设备（功能号 0）
    functions.sort_by(|a, b| {
        let fa = a
            .split('.')
            .next_back()
            .and_then(|f| f.parse::<u32>().ok())
            .unwrap_or(0);
        let fb = b
            .split('.')
            .next_back()
            .and_then(|f| f.parse::<u32>().ok())
            .unwrap_or(0);
        fb.cmp(&fa)
    });

    // 记录已解绑的设备和对应驱动，用于失败时回滚
    let mut unbound: Vec<(String, String)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // 解绑所有功能号（从高到低）
    for func_id in &functions {
        let func_path = pci_path.join(func_id);
        if let Ok(driver_link) = fs::read_link(func_path.join("driver")) {
            let driver_name = driver_link
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let unbind_path = format!("/sys/bus/pci/drivers/{}/unbind", driver_name);
            if let Err(e) = fs::write(&unbind_path, func_id) {
                errors.push(format!("解绑 {} 失败: {}", func_id, e));
            } else {
                unbound.push((func_id.clone(), driver_name.to_string()));
            }
        }
    }

    // 解绑阶段出错 → 尝试重新绑定已解绑设备
    if !errors.is_empty() {
        warn!("解绑过程出现错误，尝试恢复已解绑设备...");
        let mut rebind_failures: Vec<String> = Vec::new();
        for (func_id, driver_name) in unbound.iter().rev() {
            let bind_path = format!("/sys/bus/pci/drivers/{}/bind", driver_name);
            if let Err(e) = fs::write(&bind_path, func_id) {
                let msg = format!("恢复绑定 {} 失败: {}", func_id, e);
                warn!("{}", msg);
                rebind_failures.push(msg);
            }
        }
        if !rebind_failures.is_empty() {
            log::error!(
                "部分设备重绑定失败！请手动检查 lspci 状态:\n{}",
                rebind_failures.join("\n")
            );
        }
        return Err(GswitchError::Gpu(errors.join("; ")));
    }

    // 移除所有功能号（从高到低）
    for func_id in &functions {
        let remove_path = format!("/sys/bus/pci/devices/{}/remove", func_id);
        if let Err(e) = fs::write(&remove_path, "1") {
            errors.push(format!("移除 {} 失败: {}", func_id, e));
        }
    }

    // 移除阶段出错 → 尝试重新扫描 PCI 总线恢复设备
    if !errors.is_empty() {
        warn!("移除过程出现错误，尝试 rescan 恢复设备...");
        if let Err(e) = fs::write("/sys/bus/pci/rescan", "1") {
            warn!("PCI rescan 失败: {}", e);
        }
        return Err(GswitchError::Gpu(errors.join("; ")));
    }

    Ok(())
}

/// 查询 NVIDIA GPU 电源状态
pub fn query_runtime_power() -> bool {
    crate::detector::Detector::is_nvidia_online()
}

/// 根据当前模式自动配置 GPU 电源
///
/// - Nvidia/Hybrid 模式 → 开启 GPU（渲染需要独显在线）
/// - Integrated/Passthrough 模式 → 若 GPU 支持 runtime PM 则保持开机
///   （驱动自动进入低功耗状态，后续可热切换），否则关机（完全省电）
pub fn auto_power() -> Result<(), GswitchError> {
    let mode = crate::detector::Detector::query_current_mode();
    let should_power_on = if mode == GraphicsMode::Integrated || mode == GraphicsMode::Passthrough {
        crate::detector::Detector::gpu_supports_runtimepm().unwrap_or(false)
    } else {
        true
    };

    if should_power_on {
        runtime_power_on()
    } else if crate::detector::Detector::is_nvidia_online() {
        runtime_power_off()
    } else {
        Ok(())
    }
}

/// 等待 NVIDIA 驱动绑定，然后设置电源管理控制
fn apply_power_control(pci_id: &str, mode: GraphicsMode) -> Result<(), GswitchError> {
    if crate::config::is_dry_run() {
        info!(
            "[dry-run] 将设置电源控制: pci_id={}, mode={}",
            pci_id,
            mode.as_str()
        );
        return Ok(());
    }
    let driver_link = format!("/sys/bus/pci/devices/{}/driver", pci_id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    info!("等待 NVIDIA 驱动绑定: {}", pci_id);

    let mut bound = false;
    while std::time::Instant::now() < deadline {
        if let Ok(link) = fs::read_link(&driver_link)
            && link.file_name().and_then(|n| n.to_str()).is_some()
        {
            bound = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    if !bound {
        warn!("NVIDIA 驱动未在 10 秒内绑定，将继续执行");
    } else {
        info!("驱动已绑定，等待初始化...");
        std::thread::sleep(std::time::Duration::from_secs(2));
    }

    let pm_value = if mode == GraphicsMode::Nvidia {
        "on\n"
    } else {
        "auto\n"
    };
    info!("正在设置电源控制为: {}", pm_value.trim());

    let control = format!("/sys/bus/pci/devices/{}/power/control", pci_id);
    let mut file = fs::OpenOptions::new()
        .create(false)
        .truncate(false)
        .write(true)
        .open(&control)
        .map_err(|e| GswitchError::Gpu(format!("打开 {} 失败: {}", control, e)))?;

    file.write_all(pm_value.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| GswitchError::Gpu(format!("设置电源控制失败: {}", e)))?;

    Ok(())
}

// ====== 配置快照（用于切换失败时回滚）======

/// 配置快照，保存一组文件内容和服务状态以便失败时回滚
pub struct ConfigSnapshot {
    files: HashMap<String, Option<String>>, // path -> content (None = 文件不存在)
    services: HashMap<String, Option<bool>>, // service name -> is-enabled (None = 未安装/未知)
}

impl ConfigSnapshot {
    /// 保存指定路径列表的配置快照和服务状态
    pub fn save(paths: &[&str]) -> Result<Self, GswitchError> {
        let mut files = HashMap::new();
        for &path in paths {
            let content = if file_exists(path) {
                Some(read_file(path).unwrap_or_default())
            } else {
                None
            };
            files.insert(path.to_string(), content);
        }
        debug!("已保存 {} 个文件的配置快照", files.len());

        // 保存服务状态
        let mut services = HashMap::new();
        for svc in crate::config::SERVICE_SNAPSHOT_NAMES {
            services.insert(svc.to_string(), service_is_enabled(svc));
        }
        debug!("已保存 {} 个服务的状态快照", services.len());

        Ok(Self { files, services })
    }

    /// 将配置恢复到快照状态：先恢复文件，再恢复服务
    pub fn restore(&self) {
        info!("正在恢复配置快照...");

        // 1. 恢复文件
        for (path, content) in &self.files {
            match content {
                Some(data) => {
                    if let Err(e) = write_file(path, data) {
                        warn!("回滚 {} 失败: {}", path, e);
                    } else {
                        debug!("已回滚文件: {}", path);
                    }
                }
                None => {
                    if let Err(e) = remove_file(path) {
                        warn!("回滚时删除 {} 失败: {}", path, e);
                    }
                }
            }
        }

        // 2. 恢复服务状态
        for (svc, prev_state) in &self.services {
            match prev_state {
                Some(true) => {
                    if let Err(e) = toggle_service(svc, true) {
                        warn!("回滚服务 {} (enable) 失败: {}", svc, e);
                    } else {
                        debug!("已回滚服务: {} -> enabled", svc);
                    }
                }
                Some(false) => {
                    if let Err(e) = toggle_service(svc, false) {
                        warn!("回滚服务 {} (disable) 失败: {}", svc, e);
                    } else {
                        debug!("已回滚服务: {} -> disabled", svc);
                    }
                }
                None => {
                    // 快照时服务未安装或不可查询，跳过
                    debug!("跳过服务 {} 的回滚（快照时状态未知）", svc);
                }
            }
        }

        info!("配置快照恢复完成");
    }
}