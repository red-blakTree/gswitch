//! gswitch — NVIDIA Optimus 笔记本 GPU 切换工具
//!
//! 基于 system76-power 图形切换逻辑和 ftool GPU 模块设计。

#![deny(clippy::all)]

pub mod cache;
pub mod cli;
pub mod config;
pub mod detector;
pub mod error;
pub mod graphics;
pub mod helper;
