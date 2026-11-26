//! 平台端口：核心经由这些 trait 触达平台差异，宿主注入实现（`docs/ARCHITECTURE.md` §5）。
//!
//! 占位归属：T9-A（仓储）、T9-D（进程启动器与显示环境）。

pub mod launcher;
pub mod proxy;
pub mod repository;
