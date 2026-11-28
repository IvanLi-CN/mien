//! GATE-001 发布门禁：校验或生成 `release-gate.json`。
//!
//! 语义全部在 `scripts/release/gate.mjs`：固定必需 ID、PRD-ACC 只允许 pass、waiver 严重级别与
//! 到期、ACC-009 对 PERF-001..007 的依赖，以及 `source_sha` 必须等于目标提交。本命令只负责
//! 从仓库根转调它，保证 CI 与本地走同一条实现。缺报告即失败，没有 skip 分支。

use std::path::Path;

use anyhow::Context;

use crate::run_step;

/// 未给 `--out` 时 `--init` 的骨架输出，相对仓库根。
const DEFAULT_INIT_OUTPUT: &str = "release-gate.json";

/// 转发到 `scripts/release/gate.mjs`；`--check` 是发布门禁，`--init` 只生成全 missing 骨架。
pub fn run(
    root: &Path,
    check: Option<&Path>,
    init: bool,
    out: Option<&Path>,
    report: Option<&Path>,
) -> anyhow::Result<()> {
    let mut args: Vec<String> = vec!["scripts/release/gate.mjs".into()];
    if let Some(document) = check {
        args.push("--check".into());
        args.push(portable(root, document));
    }
    if init {
        args.push("--init".into());
        args.push("--out".into());
        args.push(match out {
            Some(path) => portable(root, path),
            None => DEFAULT_INIT_OUTPUT.to_owned(),
        });
    }
    if let Some(report) = report {
        args.push("--report".into());
        args.push(portable(root, report));
    }
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    run_step(root, "bun", &borrowed)
        .context("GATE-001 校验失败（缺报告、缺 ID 或证据不足均阻断发布）")
}

/// `bun` 以仓库根为工作目录运行；仓库内路径转为相对路径，仓库外保持绝对路径。
fn portable(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
