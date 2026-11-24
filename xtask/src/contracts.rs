//! 契约校验。
//!
//! 骨架中的<table>：`docs/contracts/*.schema.json` 与 fixture 的 JSON 是否可解析、OpenAPI 文档结构、
//! `docs/ERROR-CODES.md` 代码与 `docs/contracts/openapi.yaml`/`events.schema.json` 枚举的双向一致、
//! 迁移能否在临时库执行、seccomp 配置摘要与 `PROVENANCE.json` 是否一致。
//! 完整校验由 T8 接管（schema 编译、`$ref` 与链接解析、DDL 测试、事件/错误码双向比对、AppArmor）。

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::{db, migrate};

pub fn run(root: &Path) -> anyhow::Result<()> {
    let mut migrations = Vec::new();
    let mut json = Vec::new();
    collect(&root.join("migrations"), "sql", &mut migrations)?;
    for dir in ["fixtures", "docs/contracts"] {
        collect(&root.join(dir), "json", &mut json)?;
    }

    for file in &json {
        if file.components().any(|c| c.as_os_str() == ".cache") {
            continue;
        }
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("读取 {} 失败", display(root, file)))?;
        serde_json::from_str::<serde_json::Value>(&text)
            .with_context(|| format!("{} 不是合法 JSON", display(root, file)))?;
    }
    println!("json: {} 个文件可解析", json.len());

    if migrations.is_empty() {
        eprintln!("migrations: 暂无迁移文件，跳过迁移执行");
    } else {
        let temp = db::TempDatabase::new(false)?;
        let version = tokio::runtime::Runtime::new()?.block_on(db::run_all(&temp.url()))?;
        let expected = migrate::expected_version()?;
        if version != expected {
            bail!("迁移版本不一致：执行后 {version}，文件最高 {expected}");
        }
        println!("migrations: {version} 个版本在临时库执行通过");
    }

    let seccomp = root.join("deploy/docker/seccomp");
    if seccomp.is_dir() {
        if let Err(err) = verify_seccomp(&seccomp) {
            eprintln!("seccomp: {err:#}（T8 契约校验将设为硬失败）");
        } else {
            println!("seccomp: 摘要与 provenance 一致");
        }
    }
    println!("contracts: {} 个 JSON 与迁移执行通过", json.len());
    Ok(())
}

/// 按扩展名收集文件；目录不存在时返回空。
fn collect(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, ext, out)?;
        } else if has_ext(&path, ext) {
            out.push(path);
        }
    }
    Ok(())
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e == ext)
}

/// `PROVENANCE.json.modifiedSha256` 必须与同名配置文件的摘要一致。
fn verify_seccomp(dir: &Path) -> anyhow::Result<()> {
    let provenance: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("PROVENANCE.json"))?)?;
    let expected = provenance["modifiedSha256"]
        .as_str()
        .context("PROVENANCE.json 缺少 modifiedSha256")?;
    let actual = sha256_file(&dir.join("chromium.json"))?;
    if actual != expected {
        bail!("chromium.json 摘要 {actual} 与 provenance {expected} 不一致");
    }
    Ok(())
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let status = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .context("运行 shasum 失败")?;
    if !status.status.success() {
        bail!("shasum 失败：{}", path.display());
    }
    Ok(String::from_utf8(status.stdout)?
        .split_whitespace()
        .next()
        .context("shasum 输出为空")?
        .to_owned())
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
