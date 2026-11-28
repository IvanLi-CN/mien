use std::path::Path;

use anyhow::Context;

use crate::run_step;

/// Run the offline third-party license inventory generator; `--check` is the CI staleness gate.
pub fn run(root: &Path, check: bool) -> anyhow::Result<()> {
    let mut args = vec!["scripts/third-party/generate.mjs"];
    if check {
        args.push("--check");
    }
    run_step(root, "bun", &args).context("第三方许可清单生成/校验失败")
}
