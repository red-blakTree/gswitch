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

    let data = CacheData::new("PCI:1:0:0".to_string());
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

    let bad_json = r#"{"version":1,"nvidia_gpu_pci_bus":"PCI:999:0:0"}"#;
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

    helper::write_file(MODPROBE_GPU_PATH, MODPROBE_EMPTY).expect("write modprobe");
    helper::write_file(MODESET_PATH, MODESET_CONTENT).expect("write modeset");

    let modprobe_content = read_test_file(&root, MODPROBE_GPU_PATH);
    assert!(!modprobe_content.contains("blacklist"), "NVIDIA 模式不应黑名单");

    let modeset_content = read_test_file(&root, MODESET_PATH);
    assert!(modeset_content.contains("modeset=1"));
    assert!(modeset_content.contains("NVreg_UsePageAttributeTable=1"));
}

#[test]
fn test_switch_hybrid_writes_correct_configs() {
    let root = TestRoot::new();

    use config::*;

    helper::write_file(MODPROBE_GPU_PATH, MODPROBE_EMPTY).expect("write modprobe");
    helper::write_file(UDEV_PM_PATH, UDEV_PM_CONTENT).expect("write udev pm");

    let modprobe_content = read_test_file(&root, MODPROBE_GPU_PATH);
    assert!(!modprobe_content.contains("blacklist"));

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
