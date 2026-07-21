//! CLI 参数解析与调度

use crate::error::GswitchError;
use crate::graphics::{GpuController, GraphicsMode, NvidiaOptions, PowerAction, SwitchOptions};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "gswitch",
    about = "NVIDIA Optimus 笔记本 GPU 切换工具",
    version = concat!(
        env!("CARGO_PKG_VERSION"),
        "\ncommit: ",
        env!("GIT_HASH")
    ),
    arg_required_else_help = true,
)]
pub struct Cli {
    /// 预览模式：仅显示将要执行的操作，不实际修改系统
    #[arg(long, global = true, help = "预览模式：仅显示将要执行的操作，不实际修改系统")]
    pub dry_run: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// 切换到仅集成显卡（禁用 NVIDIA）
    Integrated,
    /// 将 NVIDIA GPU 绑定到 vfio-pci，直通给虚拟机使用
    Passthrough,
    /// PRIME 混合模式（按需渲染，省电）
    Hybrid {
        #[arg(long, help = "RTD3 电源管理级别 [0-3]", value_parser = clap::value_parser!(u32).range(0..=3))]
        rtd3: Option<u32>,
    },
    /// NVIDIA 独立显卡模式（高性能）
    Nvidia,
    /// 查询当前显卡模式
    Query,
    /// 检查系统是否支持显卡切换
    Switchable,
    /// 查询或控制运行时 GPU 电源
    Power {
        #[arg(value_enum)]
        action: Option<PowerActionArg>,
    },
    /// 获取推荐的默认显卡模式
    Default,
    /// 检查外接显示器是否需要 NVIDIA 独立显卡
    ExtDisplay,
    /// 检查 GPU 是否支持运行时电源管理
    RuntimePm,
    /// 重置所有 gswitch GPU 配置
    Reset,
    /// 创建 NVIDIA GPU 缓存（需要混合或计算模式）
    CacheCreate,
    /// 删除 NVIDIA GPU 缓存
    CacheDelete,
    /// 查询 NVIDIA GPU 缓存内容
    CacheQuery,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum PowerActionArg {
    On,
    Off,
    Auto,
}

impl Cli {
    pub fn run(&self) -> Result<(), GswitchError> {
        match &self.command {
            Command::Integrated => {
                Self::ensure_root()?;
                let opts = SwitchOptions {
                    mode: GraphicsMode::Integrated,
                    nvidia_opts: NvidiaOptions::default(),
                };
                GpuController::switch_mode(opts)
            }
            Command::Passthrough => {
                Self::ensure_root()?;
                let opts = SwitchOptions {
                    mode: GraphicsMode::Passthrough,
                    nvidia_opts: NvidiaOptions::default(),
                };
                GpuController::switch_mode(opts)
            }
            Command::Hybrid { rtd3 } => {
                Self::ensure_root()?;
                let opts = SwitchOptions {
                    mode: GraphicsMode::Hybrid,
                    nvidia_opts: NvidiaOptions {
                        rtd3: *rtd3,
                    },
                };
                GpuController::switch_mode(opts)
            }
            Command::Nvidia => {
                Self::ensure_root()?;
                let opts = SwitchOptions {
                    mode: GraphicsMode::Nvidia,
                    nvidia_opts: NvidiaOptions::default(),
                };
                GpuController::switch_mode(opts)
            }
            Command::Query => {
                println!("{}", GpuController::query_mode().as_str());
                Ok(())
            }
            Command::Switchable => {
                if GpuController::can_switch()? {
                    println!("可切换");
                } else {
                    println!("不可切换");
                }
                Ok(())
            }
            Command::Power { action } => match action {
                Some(PowerActionArg::On) => {
                    Self::ensure_root()?;
                    GpuController::power(PowerAction::On)
                }
                Some(PowerActionArg::Off) => {
                    Self::ensure_root()?;
                    GpuController::power(PowerAction::Off)
                }
                Some(PowerActionArg::Auto) => {
                    Self::ensure_root()?;
                    GpuController::power(PowerAction::Auto)
                }
                None => {
                    if GpuController::query_power() {
                        println!("开（独立显卡）");
                    } else {
                        println!("关（独立显卡）");
                    }
                    Ok(())
                }
            },
            Command::Default => {
                let mode = GpuController::get_default()?;
                println!("{}", mode.as_str());
                Ok(())
            }
            Command::ExtDisplay => {
                let requires = GpuController::external_display_requires_nvidia()?;
                if requires {
                    println!("需要独立显卡");
                } else {
                    println!("不需要独立显卡");
                }
                Ok(())
            }
            Command::RuntimePm => {
                let supports = GpuController::supports_runtimepm()?;
                if supports {
                    println!("支持");
                } else {
                    println!("不支持");
                }
                Ok(())
            }
            Command::Reset => {
                Self::ensure_root()?;
                GpuController::reset()
            }
            Command::CacheCreate => {
                Self::ensure_root()?;
                GpuController::cache_create()
            }
            Command::CacheDelete => {
                Self::ensure_root()?;
                GpuController::cache_delete()
            }
            Command::CacheQuery => {
                println!("{}", GpuController::cache_query()?);
                Ok(())
            }
        }
    }

    fn ensure_root() -> Result<(), GswitchError> {
        if crate::config::is_dry_run() {
            return Ok(());
        }

        // 优先通过 /proc/self/status 检测有效用户 ID
        let from_proc_status = || -> Option<bool> {
            let status = std::fs::read_to_string("/proc/self/status").ok()?;
            let euid = status
                .lines()
                .find(|l| l.starts_with("Uid:"))?
                .split_whitespace()
                .nth(2)?;
            Some(euid == "0")
        };

        // 备选：通过 id -u 命令检测（应对容器或 hidepid 环境）
        let from_id_command = || -> Option<bool> {
            let out = std::process::Command::new("id")
                .arg("-u")
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Some(uid == "0")
        };

        let is_root = from_proc_status()
            .or_else(from_id_command)
            .unwrap_or(false);

        if is_root {
            Ok(())
        } else {
            Err(GswitchError::NotRoot)
        }
    }
}