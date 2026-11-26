# 指纹规格实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 部分实现（M2）：规格校验、种子派生、argv 构建与清单门禁已落地；导入导出、探针矩阵、规格迁移与内核迁移未实现。
- Lifecycle: active
- Catalog note: 指纹 schema、能力清单与参数构建器的契约已建立；窗口尺寸、可选 `spoof`、`sandbox_kind`、`network_controls=unsupported` 与安装记录双摘要语义已纳入契约。

## Implementation Coverage

- Requirement coverage: `mien-core` 的 fingerprint 模块覆盖 `REQ-FP-001` 至 `REQ-FP-020` 的校验与 argv 子集：清单驱动的 `validate_spec_with_manifest`、`FingerprintSpec::fresh`/`duplicate`、CSPRNG 种子与 `derive_traits`、确定性 `build_launch_argv`、`LaunchPlan { argv, window, network }` 的 `Declared/Unverified/Unsupported` 三态。
- Verification commands: 可复跑的证据只在 `## M2 slice evidence` 列出的命令里；探针、导入导出与迁移没有对应命令，相关 VER 保持 `pending`。
- Rollout facts: 首个实现基于 fingerprint-chromium 148 的能力清单；宿主绑定字段不进入规格；Linux 的 `sandbox_kind=namespace` 需要强制 seccomp profile，Windows 清单把 DoH/IPv6 控制声明为 `unsupported` 作为有效的能力缺失 fixture。

## Coverage / rollout summary

- 三平台清单（macOS arm64 / Linux x86_64 / Windows x64）都已冻结，`fingerprint-brand` 只声明 `Edge`、`Opera`，`brand_version` 只声明 `148.0.0.0`。
- `docs/spikes/network` 的实测把直连 headful 的 `ipv6_mode` 从 `block` 降级为 `Unverified`；代理链路尚未测量。

## Remaining Gaps

- 导入导出、规格迁移（含损坏规格）与内核 148→候选迁移均未实现，`VER-FP-002/013/014/015` 因此没有证据。
- 探针矩阵未建立：UA-CH 一致性、宿主绑定字段归因、冷链路上的 DoH/IPv6 实测、沙箱启动顺序都缺。
- 缺少守护“档案 payload、启动偏好、导出与数据库均无顶层 `window`/`window_json`”的测试；目前只靠迁移文件与领域结构不含该字段这一事实。
- `ALLOW_UNSANDBOXED`、`sandbox_kind=none` 分支与审计事件未实现；非代理 UDP 的高级关闭也没有 argv 开关（`--disable-non-proxied-udp` 在仓库中不存在）。

## Verification Status

Verification Status 使用 `pending|pass|fail|blocked`。本表的 `pass` 只表示“仓库内存在可运行命令且 T19 亲自运行通过”，并写出确切命令；其余保持 `pending` 并写明缺什么。发布门禁只接受 pass 或有效 waiver。

| VER | 状态 | 计划命令与 fixture |
| --- | --- | --- |
| `VER-FP-001` | `pending` | 合法/非法规格、未知字段、缺失 `window` 与 golden argv（含单个 `window-size`）已由 `cargo test -p mien-core --lib fingerprint` 断言；缺“档案 payload、启动偏好、导出与数据库无顶层 `window`/`window_json`”的测试，导出未实现。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-002` | `pending` | 创建/复制的非零新种子与“不存在 `FINGERPRINT_SEED_CONFLICT`”由 `cargo test -p mien-core --test fingerprint_consistency` 覆盖；导入未实现，无法验证“导入保留导出种子并可复现”。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-003` | `pending` | 现有断言是 3–4 次逐字节相同（`golden_argv_matches_exactly_and_builds_repeatably`、`repeated_launches_of_one_profile_produce_byte_identical_argv`），未做 100 次，也未改变时钟与区域设置。fixture 根 `fixtures/fingerprint/golden-argv/` |
| `VER-FP-004` | `pending` | 无浏览器探针；平台与 UA-CH 一致性只在 schema 层，未在真实内核验证。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-005` | `pending` | 只有内联的 `brand_version` 主版本不得高于清单内核主版本；没有三平台品牌版本矩阵，清单也只声明 `148.0.0.0`。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-006` | `pending` | `validate_spec_with_manifest` 校验空语言、格式与重复项，但没有对应用例，也没有规范化与代理地区冲突警告。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-007` | `pending` | 只断言 `hardware_concurrency=8` 被拒、`12` 通过；缺 0/1/2/4/16/64/256 边界，也没有 `per_platform_values`、仅 `values`、两者皆缺三种清单的 fixture。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-008` | `pending` | `FingerprintSpec::fresh(platform)` 存在；缺“跨系统规格带降低保证标记”与“未确认时拒绝启动”的实现和测试。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-009` | `pending` | `window-size` 缺失、候选越界与 `network_controls` 缺失已被拒（`--lib fingerprint`、fixtures verify）；缺“无效果开关不出现在 UI”和 `sandbox_kind=none` 分支。fixture 根 `fixtures/fingerprint/negative-capabilities.json` |
| `VER-FP-010` | `pending` | 没有可写的指纹 API 面；宿主绑定字段只读、字体/插件 host-bound 限制说明均未实现。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-011` | `pending` | `linux-spoof-toggles` golden 已断言逐类 `--disable-spoofing=audio,canvas,gpu`、省略 `spoof` 即全启用、`font`/`clientrects` 被拒；缺“全部关闭时出现警告”。fixture 根 `fixtures/fingerprint/golden-argv/` |
| `VER-FP-012` | `pending` | argv 只出现一次 `window-size`、未知凭据字段被拒已断言；未覆盖含空格/引号/Unicode 的路径，也没有“伪造档案字段提供窗口尺寸”的负例。fixture 根 `fixtures/fingerprint/golden-argv/` |
| `VER-FP-013` | `pending` | 无规格迁移与损坏规格处理路径。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-014` | `pending` | 无内核迁移预检、回滚记录与探针复核。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-015` | `pending` | 导出/导入未实现，无法验证三要素一致与重新导入复现。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-016` | `pending` | 无 explicit/derived/host-bound 的探针分类实测；`derive_traits` 只是在单元测试里验证为种子的纯函数。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-017` | `pending` | 实测与规格声明冲突：pinned 148 没有任何 IPv6 阻断开关，直连 headful 下 IPv6 走真实出口（`docs/spikes/network`），因此代码按 `Enforcement::Unverified` 呈现而非强制阻断；`unsupported` 清单被拒有测试，但“声明并生效时无 DoH/IPv6 绕过”不成立。fixture 根 `fixtures/fingerprint/negative-capabilities.json` |
| `VER-FP-018` | `pending` | 非代理 UDP 的高级关闭与审计未实现；argv 中不存在 `--disable-non-proxied-udp`。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-019` | `pending` | 只有 macOS `probe_macos_sandbox`；`ALLOW_UNSANDBOXED`、`sandbox_kind=none`、审计与“沙箱校验先于代理资源”的顺序断言均未实现。fixture 根 `fixtures/fingerprint/` |
| `VER-FP-020` | `pending` | `missing_kernel_has_stable_code_in_message` 只覆盖 `KERNEL_NOT_FOUND`；没有 `KERNEL_CHECKSUM_MISMATCH` 用例，且 `kernel::registry::verify_binary` 仍是 `todo!()`。fixture 根 `fixtures/fingerprint/` |

## M1 slice evidence

- Delivered: manifest-driven `validate_spec`, canonical seed generation, `FingerprintSpec::fresh`, deterministic `build_launch_argv`, and `LaunchPlan { argv, window, network }` with `Declared/Unverified/Unsupported` enforcement states.
- Commands: `cargo test -p mien-core --lib fingerprint`, `cargo run -q -p xtask -- check`.
- Not delivered yet: cross-platform profile alignment presets and the full probe matrix; those VER rows stay `pending`.

## M2 slice evidence

- Re-run in T19 on 2026-11-27, macOS 27.2 / arm64:
  - `cargo test -p mien-core --lib fingerprint` — 15 passed（golden argv、窗口候选、负能力、种子派生）。
  - `cargo test -p mien-core --test fingerprint_consistency` — 9 passed（逐字节 argv、种子唯一、复制重抽）。
  - `cargo run -q -p xtask -- check --skip-host` — 文档与契约检查通过。
- These commands back only the `pending` notes above; no VER row meets its full pass condition yet.

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../contracts/manifests/`](../../contracts/manifests/)
- [`../../contracts/fingerprint-spec.schema.json`](../../contracts/fingerprint-spec.schema.json)
- [`../../contracts/kernel-manifest.schema.json`](../../contracts/kernel-manifest.schema.json)
- [`../../UI-SURFACES.md`](../../UI-SURFACES.md)

最近同步：M2（T19），依据 `cargo test -p mien-core --lib fingerprint`、`cargo test -p mien-core --test fingerprint_consistency`、`cargo run -q -p xtask -- check --skip-host`。
