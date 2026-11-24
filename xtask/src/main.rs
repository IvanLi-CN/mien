//! 仓库任务：`check`、`migrate`、`contracts`、`check-deny-exceptions`、`fixtures fetch|verify`。
//!
//! 全部命令以仓库根为工作目录运行；任一步非零退出即整体失败（不使用 `continue-on-error`）。
//! 迁移只作用于临时 SQLite 库，除非显式给出 `--database`。

mod check;
mod contracts;
mod db;
mod deny_exceptions;
mod fixtures;
mod licenses;
mod migrate;

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Mien 仓库任务")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 串行执行 fmt、clippy、test、deny、migrate、contracts 和 Web 检查。
    Check,
    /// 对临时库执行全部迁移。
    Migrate {
        /// 目标库；省略则新建临时库并在结束后删除。
        #[arg(long)]
        database: Option<PathBuf>,
    },
    /// 契约校验：JSON 解析、OpenAPI 结构、错误码目录交叉引用。
    Contracts,
    /// 校验 deny.toml 中每个 advisory exception 的说明与本地 advisory database 记录。
    CheckDenyExceptions,
    /// 生成或校验 NOTICE、LICENSES/ 和第三方许可报告。
    Licenses {
        /// 只校验已生成产物，不写文件。
        #[arg(long)]
        check: bool,
    },
    /// 测试 fixture。
    Fixtures {
        #[command(subcommand)]
        command: fixtures::FixturesCommand,
    },
}

fn main() -> anyhow::Result<()> {
    let root = std::env::var_os("MIEN_REPO_ROOT")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(repo_root)?;
    match Cli::parse().command {
        Command::Check => check::run(&root),
        Command::Migrate { database } => migrate::run(&root, database.as_deref()),
        Command::Contracts => contracts::run(&root),
        Command::CheckDenyExceptions => deny_exceptions::run(&root),
        Command::Licenses { check } => licenses::run(&root, check),
        Command::Fixtures { command } => fixtures::run(&root, &command),
    }
}

/// 仓库根：`xtask` crate 的上一级。
fn repo_root() -> anyhow::Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask 不在仓库根的子目录中")
        .map(Path::to_path_buf)
}

/// 执行外部命令；非零退出即失败，并回显状态码。
pub fn run_step(root: &Path, program: &str, args: &[&str]) -> anyhow::Result<()> {
    eprintln!("==> {program} {}", args.join(" "));
    let status = std::process::Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("无法启动 {program}，确认已安装"))?;
    if !status.success() {
        bail!("{program} {} 失败：{status}", args.join(" "));
    }
    Ok(())
}
