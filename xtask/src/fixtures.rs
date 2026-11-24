//! 测试 fixture 缓存管理。约定见 `docs/TEST-FIXTURES.md` §0.1。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Subcommand;
use serde::Deserialize;

const CATEGORIES: [&str; 13] = [
    "kernel",
    "fingerprint",
    "profile",
    "proxy",
    "dns",
    "network",
    "xray",
    "engine",
    "runtime",
    "api",
    "events",
    "db",
    "ui",
];

#[derive(Subcommand)]
pub enum FixturesCommand {
    /// 按 provenance 下载缺失的 fixture。
    Fetch,
    /// 校验 fixture 目录、摘要和 provenance 覆盖。
    Verify,
}

#[derive(Deserialize)]
struct Provenance {
    #[serde(default)]
    categories: BTreeMap<String, Category>,
    #[serde(default)]
    files: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct Category {
    owner: String,
    origin: String,
    fixture_version: String,
    source_url: String,
    #[serde(default)]
    upstream_version: Option<String>,
    #[serde(default)]
    commit: Option<String>,
    license: String,
    retrieved_at: String,
    fetch_command: String,
    notes: String,
    tree_sha256: String,
    fixtures: Vec<Fixture>,
}

#[derive(Deserialize)]
struct Fixture {
    path: String,
    origin: String,
    license: String,
    content_sha256: String,
    size: u64,
}

#[derive(Deserialize)]
struct Entry {
    url: String,
    sha256: String,
}

pub fn run(root: &Path, command: &FixturesCommand) -> anyhow::Result<()> {
    let fixture_root = root.join("fixtures");
    let provenance = load(&fixture_root)?;
    match command {
        FixturesCommand::Verify => verify_layout(&fixture_root, &provenance),
        FixturesCommand::Fetch => fetch(&fixture_root, &provenance),
    }
}

fn verify_layout(fixture_root: &Path, provenance: &Provenance) -> anyhow::Result<()> {
    let mut failures = 0usize;
    let mut expected = BTreeSet::new();
    for category in CATEGORIES {
        let path = fixture_root.join(category);
        let mut category_failures = Vec::new();
        if !path.is_dir() {
            category_failures.push("目录缺失".to_owned());
        }
        let readme = path.join("README.md");
        if !readme.is_file() {
            category_failures.push("README.md 缺失".to_owned());
        }
        let Some(record) = provenance.categories.get(category) else {
            category_failures.push("provenance 条目缺失".to_owned());
            print_result(category, &category_failures);
            failures += 1;
            continue;
        };
        if record.owner.is_empty()
            || record.origin != "authored"
            || record.fixture_version.is_empty()
            || record.source_url.is_empty()
            || record.upstream_version.is_none() && record.commit.is_none()
            || record.license != "Apache-2.0"
            || record.retrieved_at.is_empty()
            || record.fetch_command.is_empty()
            || record.notes.is_empty()
            || record.tree_sha256.len() != 64
        {
            category_failures.push("provenance 字段不完整".to_owned());
        }
        match tree_digest(&path)? {
            actual if actual == record.tree_sha256 => {}
            actual => {
                category_failures.push(format!("目录摘要不符 {actual} != {}", record.tree_sha256))
            }
        }
        if record.fixtures.is_empty() {
            category_failures.push("authored fixture 条目缺失".to_owned());
        }
        for fixture in &record.fixtures {
            expected.insert(fixture.path.clone());
            let fixture_path = fixture_root
                .parent()
                .unwrap_or(fixture_root)
                .join(&fixture.path);
            if fixture.origin != "authored"
                || fixture.license != "Apache-2.0"
                || fixture.content_sha256.len() != 64
            {
                category_failures.push(format!("{} provenance 字段不完整", fixture.path));
                continue;
            }
            match digest(&fixture_path)? {
                Some(actual) if actual == fixture.content_sha256 => {}
                Some(actual) => category_failures.push(format!(
                    "{} 摘要不符 {actual} != {}",
                    fixture.path, fixture.content_sha256
                )),
                None => category_failures.push(format!("{} 文件缺失", fixture.path)),
            }
            if fixture.size
                != std::fs::metadata(&fixture_path)
                    .map(|m| m.len())
                    .unwrap_or(0)
            {
                category_failures.push(format!("{} 大小不符", fixture.path));
            }
        }
        print_result(category, &category_failures);
        failures += usize::from(!category_failures.is_empty());
    }
    for path in authored_files(fixture_root)? {
        if path.file_name().is_some_and(|name| name == "README.md")
            || path
                .file_name()
                .is_some_and(|name| name == "PROVENANCE.json")
            || path.ends_with("provenance.lock.json")
        {
            continue;
        }
        let relative = path
            .strip_prefix(fixture_root.parent().unwrap_or(fixture_root))
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if !expected.contains(&relative) {
            eprintln!("FAIL: {relative}: provenance 条目缺失");
            failures += 1;
        }
    }
    if failures > 0 {
        bail!("fixtures verify 失败：{failures} 个目录/文件失败");
    }
    println!("fixtures verify: 13 个目录全部通过");
    Ok(())
}

fn print_result(category: &str, failures: &[String]) {
    if failures.is_empty() {
        println!("PASS {category}");
    } else {
        println!("FAIL {category}: {}", failures.join("; "));
    }
}

fn authored_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    Ok(files)
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !path.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .file_name()
            .is_some_and(|name| name == ".cache" || name == ".tmp")
        {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn tree_digest(category_path: &Path) -> anyhow::Result<String> {
    let mut files = authored_files(category_path)?;
    files.sort();
    let mut tree = String::new();
    for path in files {
        let relative = path
            .strip_prefix(category_path)
            .context("fixture 路径不在分类目录内")?
            .to_string_lossy()
            .replace('\\', "/");
        let digest = digest(&path)?.context("fixture 文件摘要缺失")?;
        tree.push_str(&relative);
        tree.push(' ');
        tree.push_str(&digest);
        tree.push('\n');
    }
    sha256_bytes(tree.as_bytes())
}

fn sha256_bytes(bytes: &[u8]) -> anyhow::Result<String> {
    let mut child = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("建立 shasum 输入")?
        .write_all(bytes)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("shasum 失败");
    }
    Ok(String::from_utf8(output.stdout)?
        .split_whitespace()
        .next()
        .context("shasum 输出为空")?
        .to_owned())
}

fn fetch(fixture_root: &Path, provenance: &Provenance) -> anyhow::Result<()> {
    if provenance.files.is_empty() {
        println!("fixtures: provenance files 为空");
        return Ok(());
    }
    let cache = fixture_root.join(".cache");
    let mut failures = 0usize;
    for (relative, entry) in &provenance.files {
        let target = cache.join(relative);
        if digest(&target)?.as_deref() != Some(entry.sha256.as_str()) {
            std::fs::create_dir_all(target.parent().context("fixture 路径无父目录")?)?;
            download(&entry.url, &target).with_context(|| format!("下载 {relative} 失败"))?;
        }
        if digest(&target)?.as_deref() != Some(entry.sha256.as_str()) {
            eprintln!("摘要不符 {relative}");
            failures += 1;
        }
    }
    if failures > 0 {
        bail!("{failures} 个 fixture 未通过");
    }
    Ok(())
}

fn load(fixture_root: &Path) -> anyhow::Result<Provenance> {
    let path = fixture_root.join("provenance.lock.json");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("{} 缺失", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} 非法", path.display()))
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

fn download(url: &str, target: &Path) -> anyhow::Result<()> {
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
