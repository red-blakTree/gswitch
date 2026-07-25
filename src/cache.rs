//! GPU PCI 地址缓存，用于模式切换间的持久化存储

use crate::config::{self, CACHE_FILE_PATH};
use crate::error::GswitchError;
use log::debug;
use serde::{Deserialize, Serialize};
use std::fs;

const CACHE_VERSION: u32 = 2;

/// NVIDIA 设备 ID（vendor + device），用于 GPU PCIe 断电后恢复 vfio-pci 绑定信息
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NvidiaDeviceId {
    pub vendor: u16,
    pub device: u16,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CacheData {
    version: u32,
    pub nvidia_gpu_pci_bus: String,
    /// 所有 NVIDIA PCI 设备的 (vendor, device) 对，用于直通模式恢复
    #[serde(default)]
    pub nvidia_device_ids: Vec<NvidiaDeviceId>,
}

impl CacheData {
    pub fn new(nvidia_gpu_pci_bus: String, device_ids: Vec<NvidiaDeviceId>) -> Self {
        Self {
            version: CACHE_VERSION,
            nvidia_gpu_pci_bus,
            nvidia_device_ids: device_ids,
        }
    }
}

pub struct GpuCache;

impl GpuCache {
    /// 校验 PCI 总线格式 "PCI:BB:DD:F" 及各字段范围
    fn validate_pci_bus(bus: &str) -> bool {
        let parts: Vec<&str> = bus.split(':').collect();
        if parts.len() != 4 || parts[0] != "PCI" {
            return false;
        }
        let (Ok(bus_num), Ok(dev_num), Ok(func_num)) = (
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
            parts[3].parse::<u32>(),
        ) else {
            return false;
        };
        bus_num <= 255 && dev_num <= 31 && func_num <= 7
    }

    /// 将缓存数据写入 JSON 文件
    pub fn write(data: &CacheData) -> Result<(), GswitchError> {
        let json = serde_json::to_string_pretty(data)
            .map_err(|e| GswitchError::Gpu(format!("序列化缓存失败: {}", e)))?;
        debug!("写入缓存: {}", config::resolve_path(CACHE_FILE_PATH));
        crate::helper::write_file(CACHE_FILE_PATH, &json)
    }

    /// 从 JSON 文件读取缓存数据（含校验）
    pub fn read() -> Result<CacheData, GswitchError> {
        let path = config::resolve_path(CACHE_FILE_PATH);
        debug!("读取缓存: {}", path);
        let content = fs::read_to_string(&path)
            .map_err(|e| GswitchError::Gpu(format!("读取缓存失败: {}", e)))?;
        let data: CacheData = serde_json::from_str(&content)
            .map_err(|e| GswitchError::Gpu(format!("解析缓存失败: {}", e)))?;

        if data.version != CACHE_VERSION {
            return Err(GswitchError::Gpu(format!(
                "缓存版本不匹配 (期望: {}, 实际: {})",
                CACHE_VERSION, data.version
            )));
        }

        if !Self::validate_pci_bus(&data.nvidia_gpu_pci_bus) {
            return Err(GswitchError::Gpu(format!(
                "缓存中 PCI 总线格式无效: {}",
                data.nvidia_gpu_pci_bus
            )));
        }

        Ok(data)
    }

    /// 删除缓存文件
    pub fn delete() -> Result<(), GswitchError> {
        crate::helper::remove_file(CACHE_FILE_PATH)
    }

    /// 查询并返回格式化的缓存内容
    pub fn query() -> Result<String, GswitchError> {
        match Self::read() {
            Ok(data) => serde_json::to_string_pretty(&data)
                .map_err(|e| GswitchError::Gpu(format!("序列化失败: {}", e))),
            Err(_) => Ok("无缓存数据".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_pci_bus_valid() {
        assert!(GpuCache::validate_pci_bus("PCI:1:0:0"));
        assert!(GpuCache::validate_pci_bus("PCI:10:2:1"));
        assert!(GpuCache::validate_pci_bus("PCI:255:31:7"));
    }

    #[test]
    fn test_validate_pci_bus_invalid_prefix() {
        assert!(!GpuCache::validate_pci_bus("pci:1:0:0"));
        assert!(!GpuCache::validate_pci_bus("AGP:1:0:0"));
        assert!(!GpuCache::validate_pci_bus(""));
    }

    #[test]
    fn test_validate_pci_bus_invalid_format() {
        assert!(!GpuCache::validate_pci_bus("PCI:1:0"));
        assert!(!GpuCache::validate_pci_bus("PCI:1:0:0:0"));
        assert!(!GpuCache::validate_pci_bus("PCI:abc:0:0"));
    }

    #[test]
    fn test_validate_pci_bus_out_of_range() {
        assert!(!GpuCache::validate_pci_bus("PCI:256:0:0"));
        assert!(!GpuCache::validate_pci_bus("PCI:1:32:0"));
        assert!(!GpuCache::validate_pci_bus("PCI:1:0:8"));
    }
}