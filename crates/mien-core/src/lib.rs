//! Mien 共享核心：桌面宿主与服务端宿主共用的领域层。
//!
//! 本 crate 是**骨架**：模块只含公开签名占位，`todo!()` 标明由哪张编排 Task 填充。
//! 边界见 `docs/ARCHITECTURE.md` §4–5：核心不依赖 Tauri、axum 或具体数据库驱动，
//! 平台差异通过 [`ports`] 中的 trait 注入。

pub mod domain;
pub mod error;
pub mod fingerprint;
pub mod kernel;
pub mod ports;
pub mod proxy;
pub mod runtime;
pub mod storage;

pub use error::{CoreError, ErrorCode};
