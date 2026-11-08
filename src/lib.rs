//! ns-build: 现代化跨平台声明式构建系统。
//!
//! 核心流程：解析 `ns-build.toml` → 规划任务 DAG → 并行执行（增量、缓存）。

pub mod cache;
pub mod compiler;
pub mod depfile;
pub mod executor;
pub mod export;
pub mod fsutil;
pub mod graph;
pub mod install;
pub mod platform;
pub mod planner;
pub mod runner;
pub mod script;

pub use executor::{execute, ExecOptions, ExecReport};
pub use graph::{BuildGraph, Node, NodeId, TaskKind};
pub use platform::{Arch, Host, Os};
pub use script::{BuildScript, Command, Profile, Project, Target, TargetKind};
