//! GPU 模式切换控制器

use crate::cache::{CacheData, GpuCache};
use crate::config::{self, *};
use crate::detector::Detector;
use crate::error::GswitchError;

use crate::helper::{self, ConfigSnapshot};
use log::{info, warn};

/// GPU 工作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsMode {
    Integrated,
    Passthrough,
    Hybrid,
    Nvidia,
}

impl GraphicsMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Integrated => "integrated",
            Self::Passthrough => "passthrough",
            Self::Hybrid => "hybrid",
            Self::Nvidia => "nvidia",
        }
    }
}

impl std::str::FromStr for GraphicsMode {
    type Err = GswitchError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "integrated" => Ok(Self::Integrated),
            "passthrough" => Ok(Self::Passthrough),
            "hybrid" => Ok(Self::Hybrid),
            "nvidia" => Ok(Self::Nvidia),
            _ => Err(GswitchError::Input(format!("不支持的模式: {}", s))),
        }
    }
}

/// NVIDIA 专有选项
#[derive(Debug, Clone, Default)]
pub struct NvidiaOptions {
    pub rtd3: Option<u32>,
    /// NVIDIA 模式的 Coolbits 位掩码（None = 不启用）
    pub coolbits: Option<u8>,
    /// NVIDIA 模式是否启用 ForceCompositionPipeline
    pub force_comp: bool,
}

/// 模式切换选项
pub struct SwitchOptions {
    pub mode: GraphicsMode,
    pub nvidia_opts: NvidiaOptions,
}

/// 运行时电源操作
pub enum PowerAction {
    On,
    Off,
    Auto,
}

/// 解析原始 PCI ID（如 "0000:01:00.0" 或 "01:00.0"）为 "PCI:BB:DD:F" 格式
fn parse_pci_id(raw: &str) -> Result<String, GswitchError> {
    // 判断是否有 domain 前缀：3 段为有 domain，2 段则无
    let colon_parts: Vec<&str> = raw.split(':').collect();
    let without_domain = match colon_parts.len() {
        3 => colon_parts[1..].join(":"), // 去掉 domain 前缀
        2 => raw.to_string(),
        _ => {
            return Err(GswitchError::Gpu(format!(
                "PCI ID '{}' 段数异常（期望 2 或 3 段，实际 {} 段）",
                raw,
                colon_parts.len()
            )));
        }
    };
    let parts: Vec<&str> = without_domain.split(':').collect();
    if parts.len() != 2 {
        return Err(GswitchError::Gpu(format!(
            "期望 PCI ID 为 BB:DD.F 格式，实际为 '{}' (原始: '{}')",
            without_domain, raw
        )));
    }
    let dev_func: Vec<&str> = parts[1].split('.').collect();
    if dev_func.len() != 2 {
        return Err(GswitchError::Gpu(format!(
            "期望 PCI 设备.功能号格式，实际为 '{}' (原始: '{}')",
            parts[1], raw
        )));
    }
    let bus_num = u32::from_str_radix(parts[0], 16).map_err(|_| {
        GswitchError::Gpu(format!(
            "PCI ID '{}' 中总线号 '{}' 不是有效的十六进制数",
            raw, parts[0]
        ))
    })?;
    let dev_num = u32::from_str_radix(dev_func[0], 16).map_err(|_| {
        GswitchError::Gpu(format!(
            "PCI ID '{}' 中设备号 '{}' 不是有效的十六进制数",
            raw, dev_func[0]
        ))
    })?;
    let func_num = u32::from_str_radix(dev_func[1], 16).map_err(|_| {
        GswitchError::Gpu(format!(
            "PCI ID '{}' 中功能号 '{}' 不是有效的十六进制数",
            raw, dev_func[1]
        ))
    })?;
    Ok(format!("PCI:{}:{}:{}", bus_num, dev_num, func_num))
}

/// GPU 控制器 — 提供模式切换、电源管理和缓存操作
pub struct GpuController;

impl GpuController {
    /// 切换 GPU 模式（需要重启才能完全生效）
    pub fn switch_mode(opts: SwitchOptions) -> Result<(), GswitchError> {
        if !Detector::can_switch()? {
            return Err(GswitchError::Gpu("系统不支持 GPU 切换".into()));
        }

        // 切换前保存配置快照，失败时自动回滚
        let snapshot = ConfigSnapshot::save(config::SNAPSHOT_PATHS)?;

        let result = Self::do_switch(&opts);
        if let Err(ref e) = result {
            warn!("切换失败，正在回滚配置: {}", e);
            snapshot.restore();
            // 回滚后也重建 initramfs 以恢复之前的内核模块状态
            if let Err(rebuild_err) = helper::rebuild_initramfs() {
                warn!("回滚后重建 initramfs 失败: {}", rebuild_err);
            }
        }
        result
    }

    /// 执行实际的模式切换（内部使用）
    fn do_switch(opts: &SwitchOptions) -> Result<(), GswitchError> {
        info!("正在切换到 {} 模式...", opts.mode.as_str());
        helper::cleanup()?;

        match opts.mode {
            GraphicsMode::Hybrid => Self::switch_hybrid(opts.nvidia_opts.rtd3)?,
            GraphicsMode::Integrated => Self::switch_integrated()?,
            GraphicsMode::Passthrough => Self::switch_passthrough()?,
            GraphicsMode::Nvidia => Self::switch_nvidia(&opts.nvidia_opts)?,
        }

        // 写入 PRIME 独立显卡模式标志
        // off: Integrated/Passthrough 模式下 GPU 不应参与 PRIME 渲染
        // on-demand: Hybrid 模式下 PRIME 按需卸载渲染
        // on: Nvidia 模式下始终使用独立显卡
        let prime_mode = match opts.mode {
            GraphicsMode::Hybrid => "on-demand",
            GraphicsMode::Nvidia => "on",
            GraphicsMode::Passthrough | GraphicsMode::Integrated => "off",
        };
        helper::set_prime_discrete(prime_mode)?;

        // 追加挂起电源管理配置
        helper::append_sleep_config(opts.mode)?;

        // 重建 initramfs
        helper::rebuild_initramfs()?;

        info!("切换成功！请重启系统使变更生效。");
        Ok(())
    }

    /// 查询当前 GPU 模式
    pub fn query_mode() -> GraphicsMode {
        Detector::query_current_mode()
    }

    /// 检查系统是否支持 GPU 切换
    pub fn can_switch() -> Result<bool, GswitchError> {
        Detector::can_switch()
    }

    /// 重置所有 gswitch GPU 配置
    pub fn reset() -> Result<(), GswitchError> {
        info!("正在重置 GPU 配置...");
        helper::cleanup()?;
        GpuCache::delete()?;
        helper::rebuild_initramfs()?;
        info!("重置成功！请重启系统使变更生效。");
        Ok(())
    }

    /// 创建 NVIDIA GPU 缓存（需要混合或直通模式）
    pub fn cache_create() -> Result<(), GswitchError> {
        let mode = Detector::query_current_mode();
        if mode != GraphicsMode::Hybrid && mode != GraphicsMode::Passthrough {
            return Err(GswitchError::Input(
                "缓存创建需要混合或直通模式处于激活状态".into(),
            ));
        }
        Self::write_nvidia_cache()
    }

    /// 删除 GPU 缓存
    pub fn cache_delete() -> Result<(), GswitchError> {
        GpuCache::delete()
    }

    /// 查询 GPU 缓存内容
    pub fn cache_query() -> Result<String, GswitchError> {
        GpuCache::query()
    }

    /// 运行时电源控制
    pub fn power(action: PowerAction) -> Result<(), GswitchError> {
        match action {
            PowerAction::On => helper::runtime_power_on(),
            PowerAction::Off => helper::runtime_power_off(),
            PowerAction::Auto => helper::auto_power(),
        }
    }

    /// 查询运行时 GPU 电源状态
    pub fn query_power() -> bool {
        helper::query_runtime_power()
    }

    /// 获取推荐的默认显卡模式
    pub fn get_default() -> Result<GraphicsMode, GswitchError> {
        Detector::get_default()
    }

    /// 检查外接显示器是否需要 NVIDIA 独立显卡
    pub fn external_display_requires_nvidia() -> Result<bool, GswitchError> {
        Detector::external_display_requires_nvidia()
    }

    /// 检查 GPU 是否支持运行时电源管理
    pub fn supports_runtimepm() -> Result<bool, GswitchError> {
        Detector::gpu_supports_runtimepm()
    }

    // ====== 内部实现 ======

    /// 写入 NVIDIA GPU 缓存：从 sysfs 检测 PCI 地址和设备 ID，或回退到已有缓存
    /// 在 GPU 仍然在线时调用（如 integrated 切换前），可捕获所有设备 ID 用于后续恢复
    fn write_nvidia_cache() -> Result<(), GswitchError> {
        match Detector::get_nvidia_pci_id() {
            Ok(raw) => {
                let pci_bus = parse_pci_id(&raw)?;
                // GPU 在线时同时收集所有 NVIDIA 设备 ID（用于 PCIe 断电后恢复）
                let device_ids: Vec<crate::cache::NvidiaDeviceId> =
                    Detector::get_all_nvidia_ids()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(vendor, device)| crate::cache::NvidiaDeviceId { vendor, device })
                        .collect();
                GpuCache::write(&CacheData::new(pci_bus, device_ids))
            }
            Err(_) => {
                let data = GpuCache::read().map_err(|_| {
                    GswitchError::Gpu("sysfs 中未找到 NVIDIA GPU 且无可用缓存".into())
                })?;
                info!("sysfs 中未找到 NVIDIA，重用缓存的 PCI 地址和设备 ID");
                GpuCache::write(&CacheData::new(
                    data.nvidia_gpu_pci_bus,
                    data.nvidia_device_ids,
                ))
            }
        }
    }

    fn switch_integrated() -> Result<(), GswitchError> {
        if let Err(e) = Self::write_nvidia_cache() {
            warn!("保存 NVIDIA PCI 缓存失败: {}", e);
        }

        helper::configure_gpu_services(false, false, false);

        helper::write_file(MODPROBE_GPU_PATH, MODPROBE_INTEGRATED)?;
        helper::write_file(UDEV_INTEGRATED_PATH, UDEV_INTEGRATED)?;

        // modeset 配置已在 cleanup() 中移除
        Ok(())
    }

    fn switch_passthrough() -> Result<(), GswitchError> {
        // 1. 保存 NVIDIA GPU 缓存（记住 PCI 地址用于 vfio-pci 绑定）
        if let Err(e) = Self::write_nvidia_cache() {
            warn!("保存 NVIDIA PCI 缓存失败: {}", e);
        }

        // 2. 禁用 NVIDIA 相关服务 + 挂起服务
        helper::configure_gpu_services(false, false, false);

        // 3. 获取所有 NVIDIA PCI 设备 ID 用于 vfio-pci 绑定
        //    如果 GPU PCIe 已断电（如从 integrated 模式切换过来），从缓存恢复
        let nvidia_ids: Vec<(u16, u16)> = match Detector::get_all_nvidia_ids() {
            Ok(ids) if !ids.is_empty() => ids,
            _ => {
                info!("sysfs 中未检测到 NVIDIA 设备（PCIe 可能已断电），从缓存恢复设备 ID");
                let cache = GpuCache::read().map_err(|e| {
                    GswitchError::Gpu(format!(
                        "未检测到 NVIDIA GPU 设备且缓存不可用: {}",
                        e
                    ))
                })?;
                if cache.nvidia_device_ids.is_empty() {
                    return Err(GswitchError::Gpu(
                        "未检测到 NVIDIA GPU 设备且缓存中无设备 ID（请先切换到混合模式生成缓存）"
                            .into(),
                    ));
                }
                cache
                    .nvidia_device_ids
                    .into_iter()
                    .map(|d| (d.vendor, d.device))
                    .collect()
            }
        };

        // 4. 生成 modprobe 配置：黑名单 NVIDIA 驱动 + 绑定到 vfio-pci
        let ids_str: Vec<String> = nvidia_ids
            .iter()
            .map(|(vendor, device)| format!("{:04x}:{:04x}", vendor, device))
            .collect();
        let ids_line = ids_str.join(",");

        let modprobe_content = format!(
            r#"# Automatically generated by gswitch - VFIO Passthrough mode
blacklist nouveau
blacklist nvidia
blacklist nvidia-drm
blacklist nvidia-modeset
blacklist nvidia-uvm
install nouveau /bin/false
install nvidia /bin/false
install nvidia-drm /bin/false
install nvidia-modeset /bin/false
install nvidia-uvm /bin/false
options vfio-pci ids={}
"#,
            ids_line
        );
        helper::write_file(MODPROBE_GPU_PATH, &modprobe_content)?;

        // 5. 添加 vfio-pci 到 initramfs 模块
        // 注意：modeset 和 udev PM 配置已在 cleanup() 中移除
        helper::add_vfio_to_initramfs()?;

        Ok(())
    }

    fn switch_hybrid(rtd3: Option<u32>) -> Result<(), GswitchError> {
        helper::configure_gpu_services(true, false, true);

        // modeset 配置与 modprobe 合并写入同一个文件（参考 system76-power）
        let modeset_content = generate_modeset_content(rtd3);
        helper::write_file(MODPROBE_GPU_PATH, &modeset_content)?;

        helper::write_file(UDEV_PM_PATH, UDEV_PM_CONTENT)?;
        if let Err(e) = Self::write_nvidia_cache() {
            warn!("保存 NVIDIA PCI 缓存失败: {}", e);
        }
        Ok(())
    }

    fn switch_nvidia(opts: &NvidiaOptions) -> Result<(), GswitchError> {
        if let Err(e) = Self::write_nvidia_cache() {
            warn!("保存 NVIDIA PCI 缓存失败: {}", e);
        }

        helper::configure_gpu_services(true, true, true);

        // modeset 配置与 modprobe 合并写入同一个文件（参考 system76-power）
        helper::write_file(MODPROBE_GPU_PATH, MODESET_CONTENT)?;
        helper::write_xorg_nvidia_config()?;
        helper::write_nvidia_env_config()?;

        // 额外 Xorg 选项: ForceCompositionPipeline / Coolbits
        helper::write_xorg_nvidia_extra_config(opts)?;

        // Display Manager 适配: SDDM / LightDM 的 xrandr 桥接脚本
        helper::write_dm_scripts()?;


        Ok(())
    }
}

/// 生成 modeset 配置内容，支持可选的 RTD3 参数
pub(crate) fn generate_modeset_content(rtd3: Option<u32>) -> String {
    if let Some(rtd3_val) = rtd3 {
        // CLI 已校验范围为 0-3，此处为程序化调用的安全截断
        let clamped = if rtd3_val > 3 {
            warn!("RTD3 值 {} 超出有效范围 [0-3]，已截断为 3", rtd3_val);
            3
        } else {
            rtd3_val
        };
        let hex_val = format!("0x{:02x}", clamped);
        format!(
            r#"# Automatically generated by gswitch
options nvidia-drm modeset=1
options nvidia "NVreg_DynamicPowerManagement={}"
options nvidia NVreg_UsePageAttributeTable=1 NVreg_InitializeSystemMemoryAllocations=0
"#,
            hex_val
        )
    } else {
        MODESET_CONTENT.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_graphics_mode_as_str() {
        assert_eq!(GraphicsMode::Integrated.as_str(), "integrated");
        assert_eq!(GraphicsMode::Passthrough.as_str(), "passthrough");
        assert_eq!(GraphicsMode::Hybrid.as_str(), "hybrid");
        assert_eq!(GraphicsMode::Nvidia.as_str(), "nvidia");
    }

    #[test]
    fn test_graphics_mode_from_str_valid() {
        assert_eq!(
            GraphicsMode::from_str("integrated").unwrap(),
            GraphicsMode::Integrated
        );
        assert_eq!(
            GraphicsMode::from_str("passthrough").unwrap(),
            GraphicsMode::Passthrough
        );
        assert_eq!(
            GraphicsMode::from_str("hybrid").unwrap(),
            GraphicsMode::Hybrid
        );
        assert_eq!(
            GraphicsMode::from_str("nvidia").unwrap(),
            GraphicsMode::Nvidia
        );
    }

    #[test]
    fn test_graphics_mode_from_str_invalid() {
        assert!(GraphicsMode::from_str("invalid").is_err());
        assert!(GraphicsMode::from_str("").is_err());
        assert!(GraphicsMode::from_str("INTEGRATED").is_err());
    }

    #[test]
    fn test_parse_pci_id_valid() {
        assert_eq!(parse_pci_id("0000:01:00.0").unwrap(), "PCI:1:0:0");
        assert_eq!(parse_pci_id("0000:02:01.0").unwrap(), "PCI:2:1:0");
        assert_eq!(parse_pci_id("0000:ff:1f.7").unwrap(), "PCI:255:31:7");
        assert_eq!(parse_pci_id("0000:0a:0b.1").unwrap(), "PCI:10:11:1");
    }

    #[test]
    fn test_parse_pci_id_no_domain() {
        // 部分 sysfs 格式不含 domain
        assert_eq!(parse_pci_id("01:00.0").unwrap(), "PCI:1:0:0");
    }

    #[test]
    fn test_parse_pci_id_invalid_format() {
        assert!(parse_pci_id("not-a-pci-id").is_err());
        assert!(parse_pci_id("0000:01:00").is_err());
        assert!(parse_pci_id("0000:01:00.0.0").is_err());
        assert!(parse_pci_id("").is_err());
    }

    #[test]
    fn test_parse_pci_id_invalid_hex() {
        assert!(parse_pci_id("0000:gg:00.0").is_err());
        assert!(parse_pci_id("0000:01:xx.0").is_err());
        assert!(parse_pci_id("0000:01:00.z").is_err());
    }

    #[test]
    fn test_generate_modeset_content_default() {
        let content = generate_modeset_content(None);
        assert!(content.contains("options nvidia-drm modeset=1"));
        assert!(content.contains("NVreg_UsePageAttributeTable=1"));
        // 不含 RTD3 时不应有 NVreg_DynamicPowerManagement
        assert!(!content.contains("NVreg_DynamicPowerManagement"));
    }

    #[test]
    fn test_generate_modeset_content_rtd3() {
        let content = generate_modeset_content(Some(0));
        assert!(content.contains("NVreg_DynamicPowerManagement=0x00"));

        let content = generate_modeset_content(Some(2));
        assert!(content.contains("NVreg_DynamicPowerManagement=0x02"));

        // RTD3 值被限制在 3 以内
        let content = generate_modeset_content(Some(5));
        assert!(content.contains("NVreg_DynamicPowerManagement=0x03"));
    }
}