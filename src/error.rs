use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GswitchError {
    #[error("IO 错误: {0}")]
    Io(#[from] io::Error),

    #[error("GPU 操作错误: {0}")]
    Gpu(String),

    #[error("无效输入: {0}")]
    Input(String),

    #[error("进程执行错误: {0}")]
    Process(String),

    #[error("系统不支持显卡切换")]
    NotSwitchable,

    #[error("需要 root 权限，请使用 sudo")]
    NotRoot,
}