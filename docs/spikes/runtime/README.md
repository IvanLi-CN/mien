# 运行时对账、崩溃退避与并发准入实测

本 spike 记录 M2 三项退出标准的真实测量：控制面重启后对账、崩溃有界退避、10 个档案并发启动。
数字全部来自本机实测（macOS 27.2 / Apple Silicon / 32 GiB），命令与结论一并记录；没有测量支撑的
条目留空并写明原因。

## 复跑

```sh
export KERNEL=/Users/ivan/orca/workspaces/mien/t4-kernel-spike/fixtures/.cache/kernel/148/macos-arm64/Chromium.app/Contents/MacOS/Chromium

MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test reconcile -- --nocapture
MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test admission -- --nocapture

# 收尾硬要求：必须为 0
ps -axo command | grep "mien-runtime-id=rt-" | grep -v grep | wc -l
```

`MIEN_KERNEL_BIN` 未设置时两个用例都打印 `skipped: MIEN_KERNEL_BIN not set` 并跳过真实内核部分；
假进程与脚本驱动的归属/退避用例仍会执行，因此 CI 无内核环境不会静默通过。

## 10 档案并发启动

`ten_real_kernels_start_concurrently_and_all_stop`，一个 `RuntimeSupervisor`，默认准入上限 10：

| 指标 | 实测值 |
| --- | --- |
| 并发档案数 | 10（全部 `running`） |
| 计划耗时（10 次启动全部返回） | **1.29 s / 1.61 s**（两次独立运行） |
| 峰值 RSS（10 个内核主进程之和） | **1 473 056 / 1 440 912 KiB ≈ 1.37–1.40 GiB** |
| 峰值进程数（命令行含 `mien-runtime-id=rt-`） | 10–11（内核主进程；探测子进程是否命中取决于采样时刻） |
| 第 11 个启动 | `RUNTIME_CONFLICT`，无运行时记录、无进程 |
| 停止 1 个后的槽位 | 立即可用，第 11 个成功进入 `running` |
| 全部停止后残留 | **0** |

峰值进程数是 `pgrep -f mien-runtime-id=rt-` 的快照计数；内核子进程（渲染器/GPU）不继承该参数，
因此它不是内核进程树总数。RSS 只统计 10 个主进程，不含子进程，是保守下界。

预算：用例断言峰值 RSS < 4 GiB。实测 1.40 GiB 留有三倍余量；同时运行 10 个有头实例时宿主
内存压力仍可观测，因此 `max_concurrent_runtimes` 保持可配置而不是写死。

## 崩溃退避

`bounded_restart_escalates_then_fails` 用 `exit 3` 的脚本内核驱动，策略 `RestartPolicy::bounded()`：

| 项 | 实测值 |
| --- | --- |
| 退避阶梯 | 1 s → 2 s → 4 s → 8 s，共 4 次重启 |
| 阶梯总等待 | 15 s（用例断言 ≥ 15 s，实测收敛在 15–19 s） |
| 达到 `running` 的次数 | 5（初次 + 4 次重启），无第 6 次 |
| 窗口耗尽终态 | `failed`，`error_code=RUNTIME_SPAWN_FAILED` |
| 事件证据 | 4 条 `runtime.crashed`，`detail` 分别为 `restarting in 1s/2s/4s/8s` |
| 槽位释放 | `failed` 后 `admission().active() == 0` |

`explicit_stop_wins_over_pending_restart`：真实内核被 SIGKILL 后进入 `crashed`，随后显式 `stop`
得到 `stopped`，跨过第一个 1 s 间隔后仍是 `stopped`，没有新的 `running`。默认策略
`RestartPolicy::none()` 下崩溃直接留在 `crashed`，不会自动重启。

## 控制面重启对账

`real_kernel_reconciles_after_control_plane_restart`：第一个 `RuntimeSupervisor` 启动真实内核后
被直接丢弃（模拟控制面强杀），第二个监督器调用 `reconcile()`：

- 可归属残留（命令行同时含内核路径与 `runtime_id`）被终止，进程组无存活成员；
- 落定行的 `runtime.json` 被删除，对账结果每个 `runtime_id` 只有一行，无重复 `profile_id`；
- 对账后同一 `runtime_id` 可以重新启动并正常停止。

`reconcile_kills_only_attributable_leftovers` 用三个假进程钉住归属边界：

| 场景 | 期望 | 实测 |
| --- | --- | --- |
| 可归属残留 + 指向同一进程组的重复行 | 终止进程组，两行都落定为 `crashed` | pass |
| 命令行不匹配的外来进程 | 不终止，保留 `stopping` 与端口租约 | pass |
| 元数据称 `running` 但 PID 已退出 | 落定 `crashed`，删除元数据 | pass |

对账分两遍：第一遍终止可归属进程组，第二遍让「同一进程组已被另一行终止」的重复行也落定。
存活判定用 `ps -axo pgid=,stat=` 过滤 `Z` 状态，因为 `kill(-pgid, 0)` 对未回收的僵尸同样成功，
否则重复行会永久停在 `stopping`。

## 结论边界

- 只覆盖 macOS arm64 有头内核；Linux/Docker 的准入与退避未在本 spike 测量。
- 峰值 RSS 不含内核子进程，也不是 24 小时浸泡值；内存压力下的准入拒绝（`INSUFFICIENT_RESOURCES`）
  尚未实现，只有并发上限。
- 共享引擎崩溃后的重建窗口（`1s,2s,4s,8s`、原端口恢复）有单元测试与 `EngineController::rebuild`
  实现，但本机没有 `MIEN_XRAY_BIN`，端到端未实测；`docs/spikes/xray/README.md` 记录了引擎侧的强杀/重建实测。
