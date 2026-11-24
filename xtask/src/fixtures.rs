//! 测试 fixture 缓存管理。约定见 `docs/TEST-FIXTURES.md` §0.1。
//!
//! 缓存目录 `fixtures/.cache/<relative path>`；来源为 `fixtures/provenance.lock.json` 的 `files` 表
//! （相对路径 → `url` + `sha256`）。`fetch` 先校验摘要再写缓存，`verify` 只读校验、不下载。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Subcommand;
use serde::Deserialize;

#[derive(Subcommand)]
pub enum FixturesCommand {
    /// 按 provenance 下载缺失的 fixture。
    Fetch,
    /// 校验缓存内容与 provenance。
    Verify,
}

#[derive(Deserialize)]
struct Provenance {
    files: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct Entry {
    url: String,
    sha256: String,
}

pub fn run(root: &Path, command: &FixturesCommand) -> anyhow::Result<()> {
    let fixture_root = root.join("fixtures");
    let provenance = load(&fixture_root)?;
    if provenance.files.is_empty() {
        println!("fixtures: provenance 为空");
        return Ok(());
    }
    let cache = fixture_root.join(".cache");
    let mut failures = 0usize;
    for (relative, entry) in &provenance.files {
        let target = cache.join(relative);
        match command {
            FixturesCommand::Fetch => {
                if digest(&target)?.as_deref() == Some(entry.sha256.as_str()) {
                    continue;
                }
                std::fs::create_dir_all(target.parent().context("fixture 路径无父目录")?)?;
                download(&entry.url, &target).with_context(|| format!("下载 {relative} 失败"))?;
            }
            FixturesCommand::Verify => {}
        }
        match digest(&target)? {
            Some(actual) if actual == entry.sha256 => {}
            Some(actual) => {
                eprintln!("摘要不符 {relative}: {actual} != {}", entry.sha256);
                failures += 1;
            }
            None => {
                eprintln!("缓存缺失 {relative}");
                failures += 1;
            }
        }
    }
    let ok = provenance.files.len() - failures;
    println!("fixtures: {ok} 项通过，{failures} 项失败");
    if failures > 0 {
        bail!("{failures} 个 fixture 未通过");
    }
    Ok(())
}

fn load(fixture_root: &Path) -> anyhow::Result<Provenance> {
    let path = fixture_root.join("provenance.lock.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("{} 非法", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Provenance {
            files: BTreeMap::new(),
        }),
        Err(err) => Err(err.into()),
    }
}

fn digest(path: &Path) -> anyhow::Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let output = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()?;
    if !output.status.success() {
        bail!("shasum 失败：{}", path.display());
    }
    Ok(String::from_utf8(output.stdout)?
        .split_whitespace()
        .next()
        .map(str::to_owned))
}

fn download(url: &str, target: &PathBuf) -> anyhow::Result<()> {
    eprintln!("==> curl {url}");
    let status = std::process::Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--output",
        ])
        .arg(target)
        .arg(url)
        .status()?;
    if !status.success() {
        let _ = std::fs::remove_file(target);
        bail!("curl 退出 {status}");
    }
    Ok(())
}
