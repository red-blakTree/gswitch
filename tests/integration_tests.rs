//! 集成测试 — 通过临时目录模拟 / 文件系统根路径
//!
//! 使用 thread-local 路径注入，天然支持并行测试（cargo test 默认即可运行）。

use std::fs;
use std::path::Path;

use gswitch::cache::{CacheData, GpuCache};
use gswitch::config;
use gswitch::helper;

/// 设置测试根目录并返回 TempDir guard（drop 时自动清理）
struct TestRoot {
    #[allow(dead_code)]
    dir: tempfile::TempDir,
}

impl TestRoot {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        config::set_root_for_test(dir.path().to_str().unwrap());
        config::set_dry_run(false);
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        config::clear_root_for_test();
    }
}

/// 辅助：在测试根下创建目录和文件
fn create_test_file(root: &TestRoot, rel_path: &str, content: &str) {
    let full = root.path().join(rel_path.trim_start_matches('/'));
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(&full, content).expect("write test file");
}

fn read_test_file(root: &TestRoot, rel_path: &str) -> String {
    let full = root.path().join(rel_path.trim_start_matches('/'));
    fs::read_to_string(&full).unwrap_or_default()
}

fn file_exists_in_root(root: &TestRoot, rel_path: &str) -> bool {
    root.path().join(rel_path.trim_start_matches('/')).exists()
}

// ====== 文件 I/O 测试 ======

#[test]
fn test_write_and_remove_file() {
    let root = TestRoot::new();
    let path = "/etc/test-gswitch.conf";

    // 写入
    helper::write_file(path, "hello world").expect("write");
    assert!(file_exists_in_root(&root, path));
    assert_eq!(read_test_file(&root, path), "hello world");

    // 删除
    helper::remove_file(path).expect("remove");
    assert!(!file_exists_in_root(&root, path));
}

#[test]
fn test_remove_nonexistent_file_no_error() {
    let _root = TestRoot::new();
    let result = helper::remove_file("/etc/nonexistent-gswitch.conf");
    assert!(result.is_ok());
}

#[test]
fn test_atomic_write_with_parent_creation() {
    let root = TestRoot::new();
    let path = "/var/cache/gswitch/test.json";
    helper::write_file(path, "{}").expect("write");
    assert!(file_exists_in_root(&root, path));
}

// ====== dry-run 测试 ======

#[test]
fn test_dry_run_does_not_write() {
    let root = TestRoot::new();
    config::set_dry_run(true);

    let path = "/etc/dry-run-test.conf";
    helper::write_file(path, "should not appear").expect("dry-run write");
    assert!(!file_exists_in_root(&root, path), "dry-run 不应创建文件");

    config::set_dry_run(false);
}

#[test]
fn test_dry_run_remove_does_nothing() {
    let root = TestRoot::new();
    let path = "/etc/to-keep.conf";
    create_test_file(&root, path, "keep me");

    config::set_dry_run(true);
    helper::remove_file(path).expect("dry-run remove");
    // 文件应保留
    assert!(file_exists_in_root(&root, path));

    config::set_dry_run(false);
}

// ====== 缓存测试 ======

#[test]
fn test_cache_write_read_delete() {
    let _root = TestRoot::new();

    let data = CacheData::new("PCI:1:0:0".to_string(), vec![]);
    GpuCache::write(&data).expect("write cache");

    let read = GpuCache::read().expect("read cache");
    assert_eq!(read.nvidia_gpu_pci_bus, "PCI:1:0:0");

    GpuCache::delete().expect("delete cache");
    let result = GpuCache::read();
    assert!(result.is_err());
}

#[test]
fn test_cache_query_empty() {
    let _root = TestRoot::new();
    let result = GpuCache::query().expect("query");
    assert!(result.contains("无缓存数据"));
}

#[test]
fn test_cache_version_mismatch() {
    let root = TestRoot::new();
    fs::create_dir_all(root.path().join("var/cache/gswitch")).expect("create cache dir");

    // 写入旧版本
    let bad_json = r#"{"version":99,"nvidia_gpu_pci_bus":"PCI:1:0:0"}"#;
    let cache_path = root.path().join("var/cache/gswitch/gpu-cache.json");
    fs::write(&cache_path, bad_json).expect("write bad cache");

    let result = GpuCache::read();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("版本不匹配"), "应报告版本错误: {}", err);
}

#[test]
fn test_cache_invalid_pci_format_rejected() {
    let root = TestRoot::new();
    fs::create_dir_all(root.path().join("var/cache/gswitch")).expect("create cache dir");

    let bad_json = r#"{"version":2,"nvidia_gpu_pci_bus":"PCI:999:0:0"}"#;
    let cache_path = root.path().join("var/cache/gswitch/gpu-cache.json");
    fs::write(&cache_path, bad_json).expect("write bad cache");

    let result = GpuCache::read();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("无效"), "应报告 PCI 格式无效: {}", err);
}

// ====== 配置快照测试 ======

#[test]
fn test_config_snapshot_save_and_restore() {
    let root = TestRoot::new();

    let path = "/etc/test-snapshot.conf";
    create_test_file(&root, path, "original content");

    // 保存快照
    let snapshot = helper::ConfigSnapshot::save(&["/etc/test-snapshot.conf"]).expect("save");

    // 修改文件
    helper::write_file(path, "modified content").expect("modify");

    // 恢复
    snapshot.restore();

    // 验证已恢复
    assert_eq!(read_test_file(&root, path), "original content");
}

#[test]
fn test_config_snapshot_restore_nonexistent_file() {
    let root = TestRoot::new();

    let path = "/etc/was-never-there.conf";

    // 快照时文件不存在
    let snapshot = helper::ConfigSnapshot::save(&[path]).expect("save");

    // 创建文件
    helper::write_file(path, "new content").expect("create");
    assert!(file_exists_in_root(&root, path));

    // 恢复 → 应删除
    snapshot.restore();

    // 文件应被删除
    assert!(!file_exists_in_root(&root, path), "文件应被删除: {}", path);
}

// ====== 清理测试 ======

#[test]
fn test_cleanup_removes_config_files() {
    let root = TestRoot::new();

    use config::*;
    // 创建几个 gswitch 配置文件
    let paths = [
        MODPROBE_GPU_PATH,
        MODESET_PATH,
        UDEV_INTEGRATED_PATH,
        UDEV_PM_PATH,
        PRIME_DISCRETE_PATH,
    ];

    for &path in &paths {
        create_test_file(&root, path, "# test");
        assert!(file_exists_in_root(&root, path));
    }

    helper::cleanup().expect("cleanup");

    for &path in &paths {
        assert!(
            !file_exists_in_root(&root, path),
            "{} 应被清理",
            path
        );
    }
}

// ====== 模式切换配置内容测试 ======

#[test]
fn test_switch_integrated_writes_correct_configs() {
    let root = TestRoot::new();

    use config::*;

    // 模拟 switch_integrated 写入的文件
    // 注意: 不调用 switch_integrated() 因为它会调 systemctl
    helper::write_file(MODPROBE_GPU_PATH, MODPROBE_INTEGRATED).expect("write modprobe");
    helper::write_file(UDEV_INTEGRATED_PATH, UDEV_INTEGRATED).expect("write udev");

    let modprobe_content = read_test_file(&root, MODPROBE_GPU_PATH);
    assert!(modprobe_content.contains("blacklist nvidia"));
    assert!(modprobe_content.contains("blacklist nvidia-drm"));
    assert!(modprobe_content.contains("blacklist nvidia"));

    let udev_content = read_test_file(&root, UDEV_INTEGRATED_PATH);
    assert!(udev_content.contains("ATTR{remove}=\"1\""));
    assert!(udev_content.contains("0x10de"));
}

#[test]
fn test_switch_nvidia_writes_correct_configs() {
    let root = TestRoot::new();

    use config::*;

    // 合并后的写法：modeset 内容直接写入 MODPROBE_GPU_PATH（不再使用 MODPROBE_EMPTY）
    helper::write_file(MODPROBE_GPU_PATH, MODESET_CONTENT).expect("write modprobe");

    let modprobe_content = read_test_file(&root, MODPROBE_GPU_PATH);
    assert!(!modprobe_content.contains("blacklist"), "NVIDIA 模式不应黑名单");
    assert!(modprobe_content.contains("modeset=1"), "NVIDIA 模式应包含 modeset=1");
    assert!(modprobe_content.contains("NVreg_UsePageAttributeTable=1"));
}

#[test]
fn test_switch_hybrid_writes_correct_configs() {
    let root = TestRoot::new();

    use config::*;

    // 合并后的写法：modeset 内容直接写入 MODPROBE_GPU_PATH
    let modeset_hybrid = concat!(
        "# Automatically generated by gswitch\n",
        "options nvidia-drm modeset=1\n",
        "options nvidia NVreg_UsePageAttributeTable=1 NVreg_InitializeSystemMemoryAllocations=0\n",
    );
    helper::write_file(MODPROBE_GPU_PATH, modeset_hybrid).expect("write modprobe");
    helper::write_file(UDEV_PM_PATH, UDEV_PM_CONTENT).expect("write udev pm");

    let modprobe_content = read_test_file(&root, MODPROBE_GPU_PATH);
    assert!(!modprobe_content.contains("blacklist"));
    assert!(modprobe_content.contains("modeset=1"));

    let udev_pm_content = read_test_file(&root, UDEV_PM_PATH);
    assert!(udev_pm_content.contains("power/control"));
    assert!(udev_pm_content.contains("ACTION==\"bind\""));
}

#[test]
fn test_switch_passthrough_writes_vfio_modprobe() {
    let root = TestRoot::new();

    use config::*;

    // 模拟 passthrough 写入的 modprobe（含 vfio-pci）
    let vfio_content = concat!(
        "# Automatically generated by gswitch - VFIO Passthrough mode\n",
        "blacklist nouveau\n",
        "blacklist nvidia\n",
        "blacklist nvidia-drm\n",
        "blacklist nvidia-modeset\n",
        "blacklist nvidia-uvm\n",
        "install nouveau /bin/false\n",
        "install nvidia /bin/false\n",
        "install nvidia-drm /bin/false\n",
        "install nvidia-modeset /bin/false\n",
        "install nvidia-uvm /bin/false\n",
        "options vfio-pci ids=10de:1f08,10de:10f8\n",
    );
    helper::write_file(MODPROBE_GPU_PATH, vfio_content).expect("write modprobe");

    let content = read_test_file(&root, MODPROBE_GPU_PATH);
    assert!(content.contains("options vfio-pci ids="));
    assert!(content.contains("blacklist nvidia"));
}

// ====== 检测器核心函数测试 ======

/// 在测试根下创建模拟的 sysfs PCI 设备目录
fn create_sysfs_pci_device(root: &TestRoot, pci_addr: &str, vendor: &str, device: &str, class: &str) {
    let dev_path = root.path().join(format!("sys/bus/pci/devices/{pci_addr}"));
    fs::create_dir_all(&dev_path).expect("create pci device dir");
    fs::write(dev_path.join("vendor"), format!("{vendor}\n")).expect("write vendor");
    fs::write(dev_path.join("device"), format!("{device}\n")).expect("write device");
    fs::write(dev_path.join("class"), format!("{class}\n")).expect("write class");
}

#[test]
fn test_detector_can_switch_with_dual_gpu() {
    let root = TestRoot::new();

    // 模拟笔记本 chassis type（10 = 笔记本）
    let dmi_path = root.path().join("sys/class/dmi/id");
    fs::create_dir_all(&dmi_path).expect("create dmi dir");
    fs::write(dmi_path.join("chassis_type"), "10\n").expect("write chassis_type");

    // 模拟双 GPU：Intel IGD + NVIDIA dGPU
    create_sysfs_pci_device(&root, "0000:00:02.0", "0x8086", "0x9bc4", "0x030000");
    create_sysfs_pci_device(&root, "0000:01:00.0", "0x10de", "0x1f08", "0x030000");

    assert!(gswitch::detector::Detector::can_switch().expect("can_switch"));
}

#[test]
fn test_detector_can_switch_desktop_rejected() {
    let root = TestRoot::new();

    // 模拟台式机 chassis type（3 = Desktop）
    let dmi_path = root.path().join("sys/class/dmi/id");
    fs::create_dir_all(&dmi_path).expect("create dmi dir");
    fs::write(dmi_path.join("chassis_type"), "3\n").expect("write chassis_type");

    // 即使有双 GPU 也返回 false
    create_sysfs_pci_device(&root, "0000:00:02.0", "0x8086", "0x9bc4", "0x030000");
    create_sysfs_pci_device(&root, "0000:01:00.0", "0x10de", "0x1f08", "0x030000");

    assert!(!gswitch::detector::Detector::can_switch().expect("can_switch"));
}

#[test]
fn test_detector_can_switch_nvidia_removed_but_cache_exists() {
    let root = TestRoot::new();

    // 模拟笔记本 chassis type
    let dmi_path = root.path().join("sys/class/dmi/id");
    fs::create_dir_all(&dmi_path).expect("create dmi dir");
    fs::write(dmi_path.join("chassis_type"), "10\n").expect("write chassis_type");

    // 只有 Intel IGD（NVIDIA 已被 udev 移除）
    create_sysfs_pci_device(&root, "0000:00:02.0", "0x8086", "0x9bc4", "0x030000");

    // 但缓存存在
    let cache_dir = root.path().join("var/cache/gswitch");
    fs::create_dir_all(&cache_dir).expect("create cache dir");
    fs::write(
        cache_dir.join("gpu-cache.json"),
        r#"{"version":1,"nvidia_gpu_pci_bus":"PCI:1:0:0"}"#,
    )
    .expect("write cache");

    assert!(gswitch::detector::Detector::can_switch().expect("can_switch"));
}

#[test]
fn test_detector_query_current_mode_nvidia() {
    let root = TestRoot::new();

    // 模拟 /proc/modules（nvidia 模块已加载）
    let proc_path = root.path().join("proc");
    fs::create_dir_all(&proc_path).expect("create proc dir");
    fs::write(
        proc_path.join("modules"),
        "nvidia 12345678 0 - Live 0x0000000000000000\n",
    )
    .expect("write modules");

    // 模拟 /etc/prime-discrete
    let etc_path = root.path().join("etc");
    fs::create_dir_all(&etc_path).expect("create etc dir");
    fs::write(etc_path.join("prime-discrete"), "on\n").expect("write prime-discrete");

    assert_eq!(
        gswitch::detector::Detector::query_current_mode(),
        gswitch::graphics::GraphicsMode::Nvidia
    );
}

#[test]
fn test_detector_query_current_mode_hybrid() {
    let root = TestRoot::new();

    // 模拟 /proc/modules（nvidia 模块已加载）
    let proc_path = root.path().join("proc");
    fs::create_dir_all(&proc_path).expect("create proc dir");
    fs::write(
        proc_path.join("modules"),
        "nvidia_drm 12345678 0 - Live 0x0000000000000000\n",
    )
    .expect("write modules");

    // 模拟 /etc/prime-discrete（on-demand = 混合模式）
    let etc_path = root.path().join("etc");
    fs::create_dir_all(&etc_path).expect("create etc dir");
    fs::write(etc_path.join("prime-discrete"), "on-demand\n").expect("write prime-discrete");

    assert_eq!(
        gswitch::detector::Detector::query_current_mode(),
        gswitch::graphics::GraphicsMode::Hybrid
    );
}

#[test]
fn test_detector_query_current_mode_integrated() {
    let root = TestRoot::new();

    // /proc/modules 中没有 nvidia 模块
    let proc_path = root.path().join("proc");
    fs::create_dir_all(&proc_path).expect("create proc dir");
    fs::write(proc_path.join("modules"), "intel_lpss_pci 12345 0 - Live 0x...\n")
        .expect("write modules");

    assert_eq!(
        gswitch::detector::Detector::query_current_mode(),
        gswitch::graphics::GraphicsMode::Integrated
    );
}

#[test]
fn test_detector_external_display_requires_nvidia() {
    let root = TestRoot::new();

    // 模拟笔记本 chassis
    let dmi_path = root.path().join("sys/class/dmi/id");
    fs::create_dir_all(&dmi_path).expect("create dmi dir");
    fs::write(dmi_path.join("chassis_type"), "10\n").expect("write chassis_type");

    // 双 GPU
    create_sysfs_pci_device(&root, "0000:00:02.0", "0x8086", "0x9bc4", "0x030000");
    create_sysfs_pci_device(&root, "0000:01:00.0", "0x10de", "0x1f08", "0x030000");

    // 模拟机型为 oryp8（EXTERNAL_DISPLAY_REQUIRES_NVIDIA 列表中的机型）
    fs::write(dmi_path.join("product_version"), "oryp8\n").expect("write product_version");

    assert!(
        gswitch::detector::Detector::external_display_requires_nvidia()
            .expect("external_display_requires_nvidia")
    );
}

#[test]
fn test_detector_external_display_not_required() {
    let root = TestRoot::new();

    let dmi_path = root.path().join("sys/class/dmi/id");
    fs::create_dir_all(&dmi_path).expect("create dmi dir");
    fs::write(dmi_path.join("chassis_type"), "10\n").expect("write chassis_type");

    create_sysfs_pci_device(&root, "0000:00:02.0", "0x8086", "0x9bc4", "0x030000");
    create_sysfs_pci_device(&root, "0000:01:00.0", "0x10de", "0x1f08", "0x030000");

    // 模拟机型不在列表中
    fs::write(dmi_path.join("product_version"), "unknown-model\n").expect("write product_version");

    assert!(
        !gswitch::detector::Detector::external_display_requires_nvidia()
            .expect("external_display_requires_nvidia")
    );
}

#[test]
fn test_detector_get_default_nvidia_model() {
    let root = TestRoot::new();

    let dmi_path = root.path().join("sys/class/dmi/id");
    fs::create_dir_all(&dmi_path).expect("create dmi dir");
    fs::write(dmi_path.join("chassis_type"), "10\n").expect("write chassis_type");

    // bonw16 是 DEFAULT_DISCRETE_MODELS 中的机型
    fs::write(dmi_path.join("product_version"), "bonw16\n").expect("write product_version");

    create_sysfs_pci_device(&root, "0000:00:02.0", "0x8086", "0x9bc4", "0x030000");
    create_sysfs_pci_device(&root, "0000:01:00.0", "0x10de", "0x1f08", "0x030000");

    assert_eq!(
        gswitch::detector::Detector::get_default().expect("get_default"),
        gswitch::graphics::GraphicsMode::Nvidia
    );
}
