//! Validate that every cargo-deny advisory exception is justified and still exists upstream.

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicU64;

use anyhow::{Context, bail};

const OWNER: &str = "；owner: T6";
#[cfg(test)]
static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

pub fn run(root: &Path) -> anyhow::Result<()> {
    let advisory_db = advisory_db_path()?;
    if !advisory_db.is_dir() {
        bail!(
            "cargo-deny advisory database 不存在：{}；请运行 `cargo deny fetch db`",
            advisory_db.display()
        );
    }
    let root_policy = root.join("deny.toml");
    let host_policy = root.join("src-tauri/deny.toml");
    let root_source = std::fs::read_to_string(&root_policy)
        .with_context(|| format!("读取 {} 失败", root_policy.display()))?;
    let host_source = std::fs::read_to_string(&host_policy)
        .with_context(|| format!("读取 {} 失败", host_policy.display()))?;
    let total = validate(&root_source, &root.join("Cargo.lock"), &advisory_db)?
        + validate(
            &host_source,
            &root.join("src-tauri/Cargo.lock"),
            &advisory_db,
        )?;
    println!("deny exceptions: {total} advisories have metadata and database records");
    Ok(())
}

fn advisory_db_path() -> anyhow::Result<PathBuf> {
    let cargo_home = match std::env::var_os("CARGO_HOME") {
        Some(path) => PathBuf::from(path),
        None => std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME 未设置，无法定位 cargo-deny advisory database")?
            .join(".cargo"),
    };
    let databases = cargo_home.join("advisory-dbs");
    if databases.is_dir() {
        let mut candidates = std::fs::read_dir(&databases)
            .with_context(|| format!("读取 {} 失败", databases.display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        candidates.retain(|path| path.join("crates").is_dir());
        if candidates.len() == 1 {
            return Ok(candidates.remove(0));
        }
        if candidates.len() > 1 {
            bail!(
                "cargo-deny advisory database directory {} has {} candidate databases; expected one",
                databases.display(),
                candidates.len()
            );
        }
    }
    let legacy_path = cargo_home.join("advisory-db");
    if legacy_path.is_dir() {
        return Ok(legacy_path);
    }
    bail!(
        "cargo-deny advisory database 不存在：{}；请运行 `cargo deny fetch db`",
        databases.display()
    )
}

fn validate(source: &str, lockfile_path: &Path, advisory_db: &Path) -> anyhow::Result<usize> {
    let config: toml::Value = source.parse().context("deny.toml 不是合法 TOML")?;
    let ignored = config
        .get("advisories")
        .and_then(|advisories| advisories.get("ignore"))
        .and_then(toml::Value::as_array)
        .context("deny.toml 缺少 [advisories].ignore 数组")?;
    let unused_level = config
        .get("advisories")
        .and_then(|advisories| advisories.get("unused-ignored-advisory"))
        .and_then(toml::Value::as_str);
    if unused_level != Some("deny") {
        bail!("deny.toml [advisories].unused-ignored-advisory must be `deny`");
    }
    let ignored_ids = ignored
        .iter()
        .map(|value| {
            value
                .as_str()
                .context("advisories.ignore 中必须是 advisory id")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let lockfile_packages = load_lockfile_packages(lockfile_path)?;
    let lines: Vec<_> = source.lines().collect();
    for id in ignored_ids {
        validate_exception(&lines, id, advisory_db, &lockfile_packages, lockfile_path)?;
    }
    Ok(ignored.len())
}

fn load_lockfile_packages(lockfile: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let mut packages = Vec::new();
    let source = std::fs::read_to_string(lockfile)
        .with_context(|| format!("读取 {} 失败", lockfile.display()))?;
    let lock: toml::Value = source
        .parse()
        .with_context(|| format!("{} 不是合法 Cargo.lock", lockfile.display()))?;
    let Some(entries) = lock.get("package").and_then(toml::Value::as_array) else {
        bail!("{} 缺少 package 数组", lockfile.display());
    };
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(toml::Value::as_str)
            .context("Cargo.lock package entry has no name")?;
        let version = entry
            .get("version")
            .and_then(toml::Value::as_str)
            .context("Cargo.lock package entry has no version")?;
        packages.push((name.to_owned(), version.to_owned()));
    }
    Ok(packages)
}

fn validate_exception(
    lines: &[&str],
    id: &str,
    advisory_db: &Path,
    lockfile_packages: &[(String, String)],
    lockfile_path: &Path,
) -> anyhow::Result<()> {
    let entry = format!("\"{id}\",");
    let matches: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.trim() == entry).then_some(index))
        .collect();
    if matches.len() != 1 {
        bail!(
            "deny.toml ignore `{id}` must occur exactly once; found {} entries",
            matches.len()
        );
    }
    let index = matches[0];
    if index < 3 {
        bail!("deny.toml ignore `{id}` must have three metadata comments immediately above it");
    }
    if !lines[index - 3..index]
        .iter()
        .all(|line| line.trim_start().starts_with('#'))
    {
        bail!("deny.toml ignore `{id}` must have three metadata comments immediately above it");
    }
    let id_line = lines[index - 3].trim();
    let reach_line = lines[index - 2].trim();
    let resolution_line = lines[index - 1].trim();
    let metadata = parse_id_line(id_line, id, lockfile_path)?;
    if !reach_line.starts_with("# reach: ") || !reach_line.ends_with(OWNER) {
        bail!("deny.toml ignore `{id}` needs a `# reach: ...{OWNER}` comment immediately above it");
    }
    let reach = reach_line
        .strip_prefix("# reach: ")
        .unwrap()
        .trim_end_matches(OWNER)
        .trim();
    if reach.is_empty() {
        bail!("deny.toml ignore `{id}` has an empty reach explanation");
    }
    if !resolution_line.starts_with("# 解除条件: ") {
        bail!("deny.toml ignore `{id}` needs a `# 解除条件: ...` comment immediately above it");
    }
    let resolution = resolution_line.trim_start_matches("# 解除条件: ");
    if resolution.is_empty()
        || !resolution.contains("MSRV")
        || !(resolution.contains("no patched version")
            || resolution
                .chars()
                .any(|character| character.is_ascii_digit()))
    {
        bail!(
            "deny.toml ignore `{id}` removal condition must state a fix, Rust/MSRV impact, and task"
        );
    }
    validate_locked_version(&metadata, lockfile_packages, lockfile_path)
        .with_context(|| format!("deny.toml ignore `{id}`"))?;
    validate_advisory_record(id, &metadata, advisory_db)
        .with_context(|| format!("deny.toml ignore `{id}`"))
}

fn validate_locked_version(
    metadata: &Metadata,
    lockfile_packages: &[(String, String)],
    lockfile_path: &Path,
) -> anyhow::Result<()> {
    if lockfile_packages
        .iter()
        .any(|(name, version)| name == &metadata.package && version == &metadata.version)
    {
        return Ok(());
    }
    bail!(
        "exception package {} {} is missing from {} Cargo.lock",
        metadata.package,
        metadata.version,
        lockfile_path.display()
    )
}

struct Metadata {
    package: String,
    version: String,
    class: String,
}

fn parse_id_line(line: &str, expected_id: &str, _lockfile_path: &Path) -> anyhow::Result<Metadata> {
    let body = line
        .strip_prefix("# id: ")
        .with_context(|| format!("deny.toml ignore `{expected_id}` lacks its `# id` metadata"))?;
    let fields: Vec<_> = body.split(" | ").collect();
    if fields.len() != 3 || fields[0] != expected_id {
        bail!("deny.toml ignore `{expected_id}` has malformed or mismatched id metadata");
    }
    let package_version = fields[1]
        .strip_prefix("package: ")
        .context("exception metadata must include `package: <name> <version>`")?;
    let package_name = package_version
        .split_once(" (via ")
        .map_or(package_version, |(name, _)| name);
    let fields: Vec<_> = package_name.split_whitespace().collect();
    if fields.len() < 2 {
        bail!("exception package metadata must include package name and version");
    }
    let package = fields[0];
    let version = fields[1];
    if package.is_empty() || version.is_empty() {
        bail!("exception package metadata must include package name and version");
    }
    let class = body
        .rsplit(" | ")
        .next()
        .context("exception metadata must include class")?
        .strip_prefix("class: ")
        .context("exception metadata must include class")?;
    if !matches!(class, "vulnerability" | "unmaintained") {
        bail!("exception class `{class}` must be vulnerability or unmaintained");
    }
    Ok(Metadata {
        package: package.to_owned(),
        version: version.to_owned(),
        class: class.to_owned(),
    })
}

fn validate_advisory_record(
    id: &str,
    metadata: &Metadata,
    advisory_db: &Path,
) -> anyhow::Result<()> {
    let path = advisory_db
        .join("crates")
        .join(&metadata.package)
        .join(format!("{id}.md"));
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("本地 advisory-db 中找不到 {}", path.display()))?;
    let toml_block = content
        .split_once("```toml")
        .and_then(|(_, rest)| rest.split_once("```").map(|(block, _)| block))
        .context("advisory Markdown 缺少 TOML 元数据块")?;
    let record: toml::Value = toml_block.parse().context("advisory TOML 元数据非法")?;
    let advisory = record
        .get("advisory")
        .context("advisory 文件缺少 [advisory] 区块")?;
    if advisory.get("id").and_then(toml::Value::as_str) != Some(id) {
        bail!("advisory id 与 {} 不匹配", path.display());
    }
    if advisory.get("package").and_then(toml::Value::as_str) != Some(metadata.package.as_str()) {
        bail!("advisory package 与 exception metadata 不匹配");
    }
    let informational =
        advisory.get("informational").and_then(toml::Value::as_str) == Some("unmaintained");
    let actual_class = if informational {
        "unmaintained"
    } else {
        "vulnerability"
    };
    if actual_class != metadata.class {
        bail!(
            "advisory class `{actual_class}` 与 metadata class `{}` 不匹配",
            metadata.class
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;
    use std::path::PathBuf;

    const ID: &str = "RUSTSEC-2026-0459";
    const VALID: &str = "[advisories]\nunused-ignored-advisory = \"deny\"\nignore = [\n  # id: RUSTSEC-2026-0459 | package: libsqlite3-sys 0.30.1 | class: vulnerability\n  # reach: local SQLite database；owner: T6\n  # 解除条件: patched library (requires Rust 1.89; update MSRV under T6)\n  \"RUSTSEC-2026-0459\",\n]\n";
    fn write_advisory_db(dir: &std::path::Path) {
        let package = dir.join("crates/libsqlite3-sys");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join(format!("{ID}.md")),
            "```toml\n[advisory]\nid = \"RUSTSEC-2026-0459\"\npackage = \"libsqlite3-sys\"\n```\n",
        )
        .unwrap();
    }

    fn write_lockfile(root: &std::path::Path, version: &str) -> PathBuf {
        let lockfile = root.join("Cargo.lock");
        std::fs::write(
            &lockfile,
            format!(
                "version = 4\n\n[[package]]\nname = \"libsqlite3-sys\"\nversion = \"{version}\"\n"
            ),
        )
        .unwrap();
        lockfile
    }

    #[test]
    fn accepts_metadata_for_an_existing_advisory() {
        let dir = TestDirectory::new();
        write_advisory_db(&dir.0);
        let lockfile = write_lockfile(&dir.0, "0.30.1");
        assert_eq!(validate(VALID, &lockfile, &dir.0).unwrap(), 1);
    }

    #[test]
    fn rejects_exception_when_any_metadata_comment_is_missing() {
        let dir = TestDirectory::new();
        write_advisory_db(&dir.0);
        let lockfile = write_lockfile(&dir.0, "0.30.1");
        let missing_reach = VALID.replace("  # reach: local SQLite database；owner: T6\n", "");
        let error = validate(&missing_reach, &lockfile, &dir.0)
            .unwrap_err()
            .to_string();
        assert!(error.contains("three metadata comments"), "{error}");
    }

    #[test]
    fn rejects_unknown_advisory_even_when_metadata_is_present() {
        let dir = TestDirectory::new();
        let lockfile = write_lockfile(&dir.0, "0.30.1");
        let error = format!("{:#}", validate(VALID, &lockfile, &dir.0).unwrap_err());
        assert!(error.contains("本地 advisory-db 中找不到"), "{error}");
    }

    #[test]
    fn rejects_package_version_mismatch_with_cargo_lock() {
        let dir = TestDirectory::new();
        write_advisory_db(&dir.0);
        let lockfile = write_lockfile(&dir.0, "0.30.2");
        let error = format!("{:#}", validate(VALID, &lockfile, &dir.0).unwrap_err());
        assert!(error.contains("Cargo.lock"), "{error}");
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique =
                super::TEST_DIRECTORY_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mien-xtask-deny-exceptions-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
