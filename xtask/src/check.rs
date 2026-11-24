//! 本地全量检查，顺序与 `CI.md` §3 `lint`/`rust`/`web` 一致。
//!
//! Tauri keeps its standard standalone `src-tauri/` manifest and lockfile.

use std::path::Path;

use crate::run_step;

pub fn run(root: &Path) -> anyhow::Result<()> {
    run_step(root, "cargo", &["fmt", "--all", "--check"])?;
    run_step(
        root,
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run_step(root, "cargo", &["test", "--workspace"])?;
    run_step(
        root,
        "cargo",
        &[
            "fmt",
            "--manifest-path",
            "src-tauri/Cargo.toml",
            "--all",
            "--",
            "--check",
        ],
    )?;
    run_step(
        root,
        "cargo",
        &[
            "build",
            "--manifest-path",
            "src-tauri/Cargo.toml",
            "--locked",
        ],
    )?;
    run_step(
        root,
        "cargo",
        &[
            "clippy",
            "--manifest-path",
            "src-tauri/Cargo.toml",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run_step(
        root,
        "cargo",
        &[
            "test",
            "--manifest-path",
            "src-tauri/Cargo.toml",
            "--locked",
        ],
    )?;
    run_step(
        root,
        "cargo",
        &["run", "-q", "-p", "xtask", "--", "check-deny-exceptions"],
    )?;
    run_step(root, "cargo", &["deny", "check"])?;
    run_step(
        root,
        "cargo",
        &[
            "deny",
            "--config",
            "src-tauri/deny.toml",
            "--manifest-path",
            "src-tauri/Cargo.toml",
            "check",
        ],
    )?;
    run_step(
        root,
        "cargo",
        &["run", "-q", "-p", "xtask", "--", "migrate"],
    )?;
    run_step(
        root,
        "cargo",
        &["run", "-q", "-p", "xtask", "--", "contracts"],
    )?;
    run_step(
        root,
        "cargo",
        &["run", "-q", "-p", "xtask", "--", "fixtures", "verify"],
    )?;
    run_step(
        root,
        "cargo",
        &["run", "-q", "-p", "xtask", "--", "licenses", "--check"],
    )?;
    web(root)
}

/// 前端静态检查。
fn web(root: &Path) -> anyhow::Result<()> {
    if !root.join("node_modules").is_dir() {
        anyhow::bail!("缺少 node_modules，先执行 bun install");
    }
    run_step(root, "bunx", &["biome", "check", "."])?;
    run_step(root, "bun", &["run", "typecheck"])?;
    run_step(root, "bun", &["run", "test"])?;
    run_step(root, "bun", &["run", "build"])
}
