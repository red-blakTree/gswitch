//! 通过 sysfs 检测 GPU 硬件

use crate::config::{self, *};
use crate::error::GswitchError;
use crate::graphics::GraphicsMode;
use log::{debug, info, warn};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// PCI 设备条目，包含 sysfs 中的设备名和对应路径
struct PciEntry {
    name: String,
    path: PathBuf,
}

/// 不可切换的 DMI chassis type（台式机/服务器等固定设备）
/// 参考 Desktop Management Interface (DMI) Specification
/// - 3=Desktop, 4=Low Profile Desktop, 5=Pizza Box
/// - 6=Mini Tower, 7=Tower, 17=Main Server Chassis, 23=Blade Server
pub const NON_SWITCHABLE_CHASSIS_TYPES: &[u32] = &[3, 4, 5, 6, 7, 17, 23];

/// 系统休眠模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepMode {
    S0ix,
    S3,
    /// 无法检测（mem_sleep 文件不存在或格式异常）
    Unknown,
}

/// GPU 设备信息
struct GpuInfo {
    pci_id: String,
    device_id: u16,
}

/// GPU 设备检测结果
struct DetectedGpus {
    nvidia: Vec<GpuInfo>,
    amd: Vec<GpuInfo>,
    intel: Vec<GpuInfo>,
}

/// 来自 supported-gpus.json 的 NVIDIA 设备条目
#[derive(Debug, Clone, Deserialize)]
struct NvidiaDevice {
    devid: String,
    #[allow(dead_code)]
    subdeviceid: Option<String>,
    #[allow(dead_code)]
    subvendorid: Option<String>,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    legacybranch: Option<String>,
    features: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SupportedGpus {
    chips: Vec<NvidiaDevice>,
}

/// 内存缓存 supported-gpus.json 的解析结果
static SUPPORTED_GPUS_CACHE: OnceLock<Result<Vec<NvidiaDevice>, String>> = OnceLock::new();

pub struct Detector;

impl Detector {
    /// 列出 /sys/bus/pci/devices 下所有 PCI 设备条目
    fn list_pci_entries() -> Result<Vec<PciEntry>, GswitchError> {
        let pci_path_str = config::resolve_path("/sys/bus/pci/devices");
        let pci_path = Path::new(&pci_path_str);
        if !pci_path.is_dir() {
            return Err(GswitchError::Gpu(format!(
                "{} 不存在", pci_path_str
            )));
        }
        let entries: Vec<PciEntry> = fs::read_dir(pci_path)
            .map_err(|e| GswitchError::Gpu(format!("读取 PCI 目录失败: {}", e)))?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_string();
                Some(PciEntry {
                    path: e.path(),
                    name,
                })
            })
            .collect();
        Ok(entries)
    }

    /// 检测所有 GPU 设备（仅显示控制器 class 0x03xxxx）
    fn detect_all() -> Result<DetectedGpus, GswitchError> {
        let entries = Self::list_pci_entries()?;

        let mut nvidia = Vec::new();
        let mut amd = Vec::new();
        let mut intel = Vec::new();

        for entry in &entries {
            let class_str = Self::read_sysfs_attr(&entry.path, "class");
            let class = u32::from_str_radix(class_str.trim_start_matches("0x"), 16).unwrap_or(0);

            // 仅检查显示控制器（class 0x03xxxx）
            if class >> 16 != 0x03 {
                continue;
            }

            let vendor_str = Self::read_sysfs_attr(&entry.path, "vendor");
            let vendor_id =
                u16::from_str_radix(vendor_str.trim_start_matches("0x"), 16).unwrap_or(0);

            let device_str = Self::read_sysfs_attr(&entry.path, "device");
            let device_id =
                u16::from_str_radix(device_str.trim_start_matches("0x"), 16).unwrap_or(0);

            let gpu = GpuInfo {
                pci_id: entry.name.clone(),
                device_id,
            };

            match vendor_id {
                0x10DE => nvidia.push(gpu),
                0x1002 => amd.push(gpu),
                0x8086 => intel.push(gpu),
                _ => debug!("未知 GPU 厂商 0x{vendor_id:04x} 位于 {}", entry.name),
            }
        }

        Ok(DetectedGpus { nvidia, amd, intel })
    }

    /// 检查系统是否支持 GPU 切换
    pub fn can_switch() -> Result<bool, GswitchError> {
        let chassis = fs::read_to_string(config::resolve_path(
            "/sys/class/dmi/id/chassis_type",
        ))
        .unwrap_or_default();
        let chassis_type: u32 = chassis.trim().parse().unwrap_or(0);
        let is_desktop_or_server =
            NON_SWITCHABLE_CHASSIS_TYPES.contains(&chassis_type);
        if chassis_type != 0 && is_desktop_or_server {
            debug!("检测到非笔记本机型 (chassis_type={})，不可切换", chassis_type);
            return Ok(false);
        }

        let gpus = Self::detect_all()?;
        let has_nvidia = !gpus.nvidia.is_empty();
        let has_igpu = !gpus.amd.is_empty() || !gpus.intel.is_empty();

        if has_nvidia && has_igpu {
            info!("系统支持 GPU 切换");
            return Ok(true);
        }

        // 集成模式下 NVIDIA 可能已被 udev 移除；检查缓存
        let cache_path = crate::config::resolve_path(CACHE_FILE_PATH);
        if has_igpu && Path::new(&cache_path).exists() {
            info!("sysfs 中未找到 NVIDIA 但缓存存在，仍可切换");
            return Ok(true);
        }

        Ok(false)
    }

    /// 通过检查内核模块和配置文件查询当前 GPU 模式
    pub fn query_current_mode() -> GraphicsMode {
        let modules =
            fs::read_to_string(config::resolve_path("/proc/modules")).unwrap_or_default();

        let nvidia_loaded = modules.lines().any(|line| {
            let name = line.split_whitespace().next().unwrap_or("");
            matches!(
                name,
                "nvidia" | "nvidia_drm" | "nvidia_current" | "nvidia_current_drm"
            )
        });

        let prime_mode = fs::read_to_string(crate::config::resolve_path(PRIME_DISCRETE_PATH))
            .unwrap_or_default()
            .trim()
            .to_string();

        let modprobe_path = crate::config::resolve_path(MODPROBE_GPU_PATH);
        let integrated_path = crate::config::resolve_path(UDEV_INTEGRATED_PATH);

        let has_integrated_config =
            Path::new(&modprobe_path).exists() && Path::new(&integrated_path).exists();

        let modeset_path = crate::config::resolve_path(MODESET_PATH);
        let has_modeset = Path::new(&modeset_path).exists();

        let gswitch_content = fs::read_to_string(&modprobe_path).unwrap_or_default();
        // 排除注释行，仅匹配非注释行中的 vfio-pci 绑定
        let is_passthrough = gswitch_content
            .lines()
            .any(|line| !line.trim().starts_with('#') && line.contains("options vfio-pci"));

        // 优先检测直通模式
        if is_passthrough {
            return GraphicsMode::Passthrough;
        }

        // NVIDIA 模块未加载 → 集成模式
        if !nvidia_loaded {
            return GraphicsMode::Integrated;
        }

        if has_integrated_config {
            warn!(
                "配置/运行时不一致 — 集成模式配置存在但 nvidia 模块仍在运行"
            );
        }

        if prime_mode == "on" {
            return GraphicsMode::Nvidia;
        }

        if prime_mode == "on-demand" || has_modeset {
            return GraphicsMode::Hybrid;
        }

        // Fallthrough：prime_mode 值异常但 nvidia 已加载，保守按 Nvidia 处理
        warn!(
            "无法从 prime_mode='{}' 明确判断模式，nvidia 模块已加载，默认按 nvidia 处理",
            prime_mode
        );
        GraphicsMode::Nvidia
    }

    /// 读取 sysfs 属性，失败时记录 debug 日志
    fn read_sysfs_attr(dev_path: &Path, attr: &str) -> String {
        match fs::read_to_string(dev_path.join(attr)) {
            Ok(s) => s.trim().to_string(),
            Err(e) => {
                debug!("读取 sysfs {}/{} 失败: {}", dev_path.display(), attr, e);
                String::new()
            }
        }
    }

    /// 获取 NVIDIA GPU 原始 PCI ID（如 "0000:01:00.0"）
    pub fn get_nvidia_pci_id() -> Result<String, GswitchError> {
        let gpus = Self::detect_all()?;
        gpus.nvidia
            .first()
            .map(|g| g.pci_id.clone())
            .ok_or_else(|| GswitchError::Gpu("未找到 NVIDIA GPU".into()))
    }

    /// 获取所有 NVIDIA PCI 设备的 (vendor_id, device_id) 对（所有功能号，不限于显示控制器）
    pub fn get_all_nvidia_ids() -> Result<Vec<(u16, u16)>, GswitchError> {
        let entries = Self::list_pci_entries()?;

        let mut ids = Vec::new();
        for entry in &entries {
            let vendor_str = fs::read_to_string(entry.path.join("vendor")).unwrap_or_default();
            let vendor = u16::from_str_radix(vendor_str.trim_start_matches("0x"), 16).unwrap_or(0);
            if vendor != 0x10DE {
                continue;
            }

            let device_str = fs::read_to_string(entry.path.join("device")).unwrap_or_default();
            let device = u16::from_str_radix(device_str.trim_start_matches("0x"), 16).unwrap_or(0);

            ids.push((vendor, device));
        }

        if ids.is_empty() {
            return Err(GswitchError::Gpu("未找到任何 NVIDIA PCI 设备".into()));
        }

        Ok(ids)
    }

    /// 检查 NVIDIA GPU 是否在线（出现在 sysfs 中）
    pub fn is_nvidia_online() -> bool {
        Self::detect_all()
            .map(|g| !g.nvidia.is_empty())
            .unwrap_or(false)
    }

    /// 检测系统休眠模式：S0ix (s2idle) vs S3 (deep)
    pub fn detect_sleep_mode() -> SleepMode {
        let mem_sleep = fs::read_to_string(config::resolve_path("/sys/power/mem_sleep"))
            .unwrap_or_default();
        if mem_sleep.contains("[s2idle]") {
            return SleepMode::S0ix;
        }
        if mem_sleep.contains("[deep]") {
            return SleepMode::S3;
        }
        // mem_sleep 存在但没有带方括号的默认值（异常情况）
        if mem_sleep.contains("s2idle") {
            return SleepMode::S0ix;
        }
        if mem_sleep.contains("deep") {
            return SleepMode::S3;
        }
        debug!(
            "无法从 /sys/power/mem_sleep 检测休眠模式，内容: '{}'",
            mem_sleep.trim()
        );
        SleepMode::Unknown
    }

    /// 检查 NVIDIA GPU 是否支持运行时电源管理
    pub fn gpu_supports_runtimepm() -> Result<bool, GswitchError> {
        let gpus = Self::detect_all()?;
        if gpus.nvidia.is_empty() {
            return Ok(false);
        }

        let device_id = gpus.nvidia[0].device_id;
        let dev = Self::get_nvidia_device(device_id)?;
        info!("NVIDIA 设备 0x{device_id:04x} 特性: {:?}", dev.features);
        Ok(dev.features.iter().any(|f| f == "runtimepm"))
    }

    /// 加载并缓存所有 supported-gpus.json 中的 NVIDIA 设备列表
    fn load_supported_gpus() -> Result<&'static [NvidiaDevice], &'static str> {
        SUPPORTED_GPUS_CACHE
            .get_or_init(|| match Self::load_supported_gpus_inner() {
                Ok(devices) => Ok(devices),
                Err(e) => Err(e),
            })
            .as_ref()
            .map(|v| v.as_slice())
            .map_err(|e| e.as_str())
    }

    /// 实际执行磁盘扫描和 JSON 解析
    fn load_supported_gpus_inner() -> Result<Vec<NvidiaDevice>, String> {
        let doc_root = "/usr/share/doc";
        let mut supported: Vec<PathBuf> = if Path::new(doc_root).is_dir() {
            fs::read_dir(doc_root)
                .map_err(|e| format!("读取 /usr/share/doc 失败: {e}"))?
                .filter_map(Result::ok)
                .map(|f| f.path())
                .filter(|f| {
                    let s = f.to_str().unwrap_or("");
                    NVIDIA_DOC_PATTERNS.iter().any(|pat| s.contains(pat))
                })
                .map(|f| f.join("supported-gpus.json"))
                .filter(|f| f.exists())
                .collect()
        } else {
            Vec::new()
        };

        for alt_path in NVIDIA_GPU_JSON_ALT_PATHS {
            let p = Path::new(alt_path);
            if p.exists() && !supported.iter().any(|s| s == p) {
                supported.push(p.to_path_buf());
            }
        }

        if supported.is_empty() {
            return Err("未找到 supported-gpus.json（NVIDIA 驱动可能未安装）".into());
        }

        let mut all_devices: Vec<NvidiaDevice> = Vec::new();
        for json_path in &supported {
            let raw = match fs::read_to_string(json_path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let gpus: SupportedGpus = match serde_json::from_str(&raw) {
                Ok(g) => g,
                Err(_) => continue,
            };
            all_devices.extend(gpus.chips);
        }

        if all_devices.is_empty() {
            return Err("supported-gpus.json 中未解析到任何设备条目".into());
        }

        Ok(all_devices)
    }

    /// 在 supported-gpus.json 中查找 NVIDIA 设备（使用内存缓存）
    fn get_nvidia_device(id: u16) -> Result<NvidiaDevice, GswitchError> {
        let devices = Self::load_supported_gpus().map_err(|e| GswitchError::Gpu(e.to_string()))?;

        for dev in devices {
            let did = dev.devid.trim_start_matches("0x").trim();
            if let Ok(parsed) = u16::from_str_radix(did, 16)
                && parsed == id
            {
                return Ok(dev.clone());
            }
        }

        Err(GswitchError::Gpu(format!(
            "在所有 supported-gpus.json 中均未找到设备 0x{id:04x}"
        )))
    }

    /// 获取 DMI 厂商字符串
    #[allow(dead_code)]
    pub fn get_vendor() -> String {
        fs::read_to_string(config::resolve_path("/sys/class/dmi/id/sys_vendor"))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// 获取 DMI 产品版本字符串
    pub fn get_product() -> String {
        fs::read_to_string(config::resolve_path("/sys/class/dmi/id/product_version"))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// 检查外接显示器是否需要 NVIDIA 独立显卡
    ///
    /// 机型匹配时忽略大小写，避免不同 BIOS 版本间大小写不一致的问题。
    pub fn external_display_requires_nvidia() -> Result<bool, GswitchError> {
        if !Self::can_switch()? {
            return Err(GswitchError::Gpu("系统不支持 GPU 切换".into()));
        }

        let model = Self::get_product().to_lowercase();

        Ok(EXTERNAL_DISPLAY_REQUIRES_NVIDIA
            .iter()
            .any(|m| m.to_lowercase() == model))
    }

    /// 获取推荐的默认显卡模式
    pub fn get_default() -> Result<GraphicsMode, GswitchError> {
        if !Self::can_switch()? {
            return Err(GswitchError::NotSwitchable);
        }

        let product = Self::get_product().to_lowercase();
        let runtimepm = Self::gpu_supports_runtimepm().unwrap_or(false);

        // 特定机型直接走独立显卡（匹配忽略大小写）
        if DEFAULT_DISCRETE_MODELS
            .iter()
            .any(|m| m.to_lowercase() == product)
        {
            return Ok(GraphicsMode::Nvidia);
        }

        // 通用逻辑：GPU 支持 runtimepm → Hybrid；否则 → Integrated
        if runtimepm {
            Ok(GraphicsMode::Hybrid)
        } else {
            Ok(GraphicsMode::Integrated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_sleep_mode_s2idle() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let root = dir.path().to_str().unwrap().to_string();

        // 创建模拟的 /sys/power/mem_sleep（默认 s2idle）
        let sysfs_path = Path::new(&root).join("sys/power");
        std::fs::create_dir_all(&sysfs_path).expect("创建 sysfs 目录");
        std::fs::write(sysfs_path.join("mem_sleep"), "[s2idle] deep").expect("写入 mem_sleep");

        config::set_root_for_test(&root);
        assert_eq!(Detector::detect_sleep_mode(), SleepMode::S0ix);
        config::clear_root_for_test();
    }

    #[test]
    fn test_detect_sleep_mode_s3() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let root = dir.path().to_str().unwrap().to_string();

        let sysfs_path = Path::new(&root).join("sys/power");
        std::fs::create_dir_all(&sysfs_path).expect("创建 sysfs 目录");
        std::fs::write(sysfs_path.join("mem_sleep"), "s2idle [deep]").expect("写入 mem_sleep");

        config::set_root_for_test(&root);
        assert_eq!(Detector::detect_sleep_mode(), SleepMode::S3);
        config::clear_root_for_test();
    }

    #[test]
    fn test_detect_sleep_mode_unknown() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let root = dir.path().to_str().unwrap().to_string();

        // 不创建 mem_sleep 文件 → Unknown
        config::set_root_for_test(&root);
        assert_eq!(Detector::detect_sleep_mode(), SleepMode::Unknown);
        config::clear_root_for_test();
    }

    #[test]
    fn test_detect_sleep_mode_no_brackets() {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let root = dir.path().to_str().unwrap().to_string();

        // 无方括号但内容存在 → 根据内容推断
        let sysfs_path = Path::new(&root).join("sys/power");
        std::fs::create_dir_all(&sysfs_path).expect("创建 sysfs 目录");
        std::fs::write(sysfs_path.join("mem_sleep"), "s2idle deep").expect("写入 mem_sleep");

        config::set_root_for_test(&root);
        assert_eq!(Detector::detect_sleep_mode(), SleepMode::S0ix);
        config::clear_root_for_test();
    }

    #[test]
    fn test_external_display_model_list_valid() {
        for model in EXTERNAL_DISPLAY_REQUIRES_NVIDIA {
            assert!(!model.is_empty());
            assert!(!model.contains(' '), "机型名不应含空格: {}", model);
        }
    }

    #[test]
    fn test_default_discrete_models_valid() {
        for model in DEFAULT_DISCRETE_MODELS {
            assert!(!model.is_empty());
            assert!(!model.contains(' '), "机型名不应含空格: {}", model);
        }
    }

    #[test]
    fn test_read_sysfs_attr_returns_empty_on_nonexistent() {
        let path = std::path::Path::new("/nonexistent/path");
        let result = Detector::read_sysfs_attr(path, "class");
        assert!(result.is_empty());
    }
}