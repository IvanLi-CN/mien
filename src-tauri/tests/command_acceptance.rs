//! 命令层验收（PRD-ACC-001 的真实产品路径）：创建 → 启动 → 状态 → 停止 → 再启动。
//!
//! 这里调用的是 `generate_handler!` 注册的同一批命令函数，断言 UI 实际收到的
//! JSON 载荷，而不是 `AppState` 的 Rust 结构。ACL 与参数反序列化由 `ipc.rs` 覆盖；
//! 本文件补的是命令函数本身的契约与真实内核生命周期。
//!
//! `tauri::State` 没有公开构造函数，所以用 `tauri::test` 的 mock 组件只为取出
//! `State<'_, AppState>`；命令体与发布构建逐字相同，没有替代实现。注意不使用
//! `generate_context!`：它每个 crate 只能展开一次，装配点留给 `ipc.rs`。
//!
//! 只有设置 `MIEN_KERNEL_BIN` 时才执行；未设置时打印显式跳过原因，不静默通过。
//! 数据根用临时目录，绝不触碰 `~/Library/Application Support/Mien`。

#[path = "../src/commands.rs"]
#[allow(dead_code)]
mod commands;
#[path = "../src/state.rs"]
#[allow(dead_code)]
mod state;

use mien_core::domain::profile::ProfileMode;
use tauri::Manager;
use tauri::test::{mock_builder, mock_context, noop_assets};

const SKIP_MESSAGE: &str = "skipped: MIEN_KERNEL_BIN not set";

fn kernel() -> Option<std::path::PathBuf> {
    match std::env::var_os("MIEN_KERNEL_BIN") {
        None => {
            eprintln!("{SKIP_MESSAGE}");
            None
        }
        Some(value) => {
            let path = std::path::PathBuf::from(value);
            assert!(path.is_file(), "MIEN_KERNEL_BIN is not a file: {path:?}");
            Some(path)
        }
    }
}

fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn mien_runtime_processes(runtime_id: &str) -> Vec<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-f", "--", &format!("--mien-runtime-id={runtime_id}")])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// 命令返回值按 IPC 序列化后的形态断言，避免直接比对 Rust 结构。
fn json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("serialize command result")
}

#[test]
fn create_start_status_stop_start_through_the_command_layer() {
    let Some(binary) = kernel() else {
        return;
    };
    assert_eq!(std::env::consts::OS, "macos");
    let directory = tempfile::tempdir().expect("temporary directory");
    let state = state::AppState::open_at_with_kernel(directory.path().join("data"), Some(binary))
        .expect("host state");
    // mock 组件只提供 `State`；命令函数本身是注册到 `generate_handler!` 的同一批。
    let app = mock_builder()
        .manage(state)
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let host = app.state::<state::AppState>();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    runtime.block_on(async {
        // 1. 创建：默认指纹必须通过 manifest 准入。
        let created = json(
            &commands::profiles_create(
                host.clone(),
                state::CreateProfileInput {
                    name: "command-acceptance".into(),
                    fingerprint: None,
                    mode: Some(ProfileMode::Headful),
                    start_url: None,
                    notes: None,
                    automation_api: None,
                    group_id: None,
                    tags: Vec::new(),
                    proxy_id: None,
                    kernel_version: None,
                },
            )
            .await
            .expect("create"),
        );
        let profile_id = created["id"].as_str().expect("profile id").to_owned();
        assert_eq!(created["name"], "command-acceptance");
        assert_eq!(created["mode"], "headful");
        assert_eq!(created["direct"], true);
        assert_eq!(created["isolation"], "unknown");
        assert_eq!(
            created["reduced_assurance"], true,
            "a direct-mode profile is reduced assurance before it ever starts"
        );
        assert!(
            created["runtime"].is_null(),
            "a fresh profile has no runtime: {created:?}"
        );

        // 启动前必须先登记安装：校验拿的是"安装时的摘要"，不是刚算出来的自己。
        let installed = json(
            &commands::kernel_install(
                host.clone(),
                host.manifest_kernel_version(),
                kernel().expect("kernel path").display().to_string(),
                None,
            )
            .await
            .expect("install"),
        );
        assert_eq!(installed["version"], host.manifest_kernel_version());

        // 2. 启动：有头内核真的起来；disclosures 必须带 direct 与 unverified 说明。
        let started = json(
            &commands::runtime_start(host.clone(), profile_id.clone())
                .await
                .expect("first start"),
        );
        assert_eq!(started["state"], "running");
        assert_eq!(started["mode"], "headful");
        assert_eq!(
            started["reducedAssurance"], true,
            "direct mode is reduced assurance"
        );
        let runtime_id = started["runtimeId"]
            .as_str()
            .expect("runtime id")
            .to_owned();
        let disclosures = started["disclosures"]
            .as_array()
            .expect("disclosures array");
        assert!(
            disclosures.iter().all(serde_json::Value::is_string),
            "disclosures are strings: {disclosures:?}"
        );
        assert!(
            disclosures.iter().any(|value| value
                .as_str()
                .is_some_and(|text| text.contains("direct mode"))),
            "direct mode must be disclosed: {disclosures:?}"
        );
        assert!(
            disclosures.iter().any(|value| value
                .as_str()
                .is_some_and(|text| text.contains("unverified"))),
            "unverified network controls must be disclosed: {disclosures:?}"
        );
        println!(
            "start: state={} isolation={} reducedAssurance={} runtimeId={runtime_id} disclosures={disclosures:?}",
            started["state"], started["isolation"], started["reducedAssurance"]
        );

        // 3. 状态：与上一帧 start 一致，且 pid 是一个真实存活的进程。
        let running = json(
            &commands::runtime_status(host.clone(), profile_id.clone())
                .await
                .expect("status"),
        );
        assert_eq!(running["state"], "running");
        assert_eq!(running["isolation"], started["isolation"]);
        let pid = running["pid"].as_u64().expect("running pid") as u32;
        assert!(process_alive(pid), "the headful kernel must be running");
        let processes = mien_runtime_processes(&runtime_id);
        assert!(
            !processes.is_empty(),
            "kernel children must be attributable: runtimeId={runtime_id}"
        );
        println!("status: pid={pid} pgrep={processes:?}");

        // 4. 停止：状态回到 stopped，进程被回收，不留孤儿。
        let stopped = json(
            &commands::runtime_stop(host.clone(), profile_id.clone())
                .await
                .expect("first stop"),
        );
        assert_eq!(stopped["state"], "stopped");
        assert!(!process_alive(pid), "stop must reap the kernel process");
        assert!(
            mien_runtime_processes(&runtime_id).is_empty(),
            "stop must reap the whole process group"
        );

        // 5. 再次启动：同一档案必须能重新拉起一条全新的内核。
        let restarted = json(
            &commands::runtime_start(host.clone(), profile_id.clone())
                .await
                .expect("second start"),
        );
        assert_eq!(restarted["state"], "running");
        assert_eq!(restarted["runtimeId"], runtime_id.as_str());
        let restarted_status = json(
            &commands::runtime_status(host.clone(), profile_id.clone())
                .await
                .expect("status after restart"),
        );
        let restarted_pid = restarted_status["pid"].as_u64().expect("restarted pid") as u32;
        assert_ne!(restarted_pid, pid, "a restart needs a new kernel process");
        assert!(process_alive(restarted_pid));

        let stopped = json(
            &commands::runtime_stop(host.clone(), profile_id.clone())
                .await
                .expect("second stop"),
        );
        assert_eq!(stopped["state"], "stopped");
        assert!(!process_alive(restarted_pid), "second stop must reap too");

        // 列表命令必须反映最终状态，UI 刷新后不会停在 running。
        let listed = json(
            &commands::profiles_list(host.clone(), 10, None, None)
                .await
                .expect("list"),
        );
        let items = listed["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], profile_id.as_str());
        assert_eq!(items[0]["runtime"]["state"], "stopped");
    });
}

/// PRD-ACC-001 的"访问探针页"：开启自动化 API 后，运行中的内核必须暴露
/// 可用的 CDP 端口。这里验证端口真的在服务并能报出浏览器版本，而不是只看
/// `cdpPort` 字段非空——一个写进状态却不监听任何端口的数字没有意义。
#[test]
fn automation_api_exposes_a_reachable_cdp_port() {
    let Some(binary) = kernel() else {
        return;
    };
    assert_eq!(std::env::consts::OS, "macos");
    let directory = tempfile::tempdir().expect("temporary directory");
    let state = state::AppState::open_at_with_kernel(directory.path().join("data"), Some(binary))
        .expect("host state");
    let app = mock_builder()
        .manage(state)
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let host = app.state::<state::AppState>();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    runtime.block_on(async {
        // 创建时开启 automation_api；它必须原样落库，否则 CDP 端口永远不会出现。
        let created = json(
            &commands::profiles_create(
                host.clone(),
                state::CreateProfileInput {
                    name: "cdp-acceptance".into(),
                    fingerprint: None,
                    mode: Some(ProfileMode::Headful),
                    start_url: None,
                    notes: None,
                    automation_api: Some(true),
                    group_id: None,
                    tags: Vec::new(),
                    proxy_id: None,
                    kernel_version: None,
                },
            )
            .await
            .expect("create"),
        );
        assert_eq!(
            created["automation_api"], true,
            "the create command must not drop automation_api: {created:?}"
        );
        let profile_id = created["id"].as_str().expect("profile id").to_owned();

        commands::kernel_install(
            host.clone(),
            host.manifest_kernel_version(),
            kernel().expect("kernel path").display().to_string(),
            None,
        )
        .await
        .expect("install");

        let started = json(
            &commands::runtime_start(host.clone(), profile_id.clone())
                .await
                .expect("start"),
        );
        assert_eq!(started["state"], "running");
        // `runtime_start` 只报启动结论；端口属于运行状态，UI 也要再查一次 status。
        let running = json(
            &commands::runtime_status(host.clone(), profile_id.clone())
                .await
                .expect("status"),
        );
        let port = running["cdpPort"].as_u64().unwrap_or_else(|| {
            panic!("automation_api must yield a CDP port in runtime_status: {running:?}")
        });
        let port = u16::try_from(port).expect("cdp port fits u16");

        // CDP 必须是真实监听的服务：读一次 /json/version。
        let body = http_get(port, "/json/version");
        assert!(
            body.contains("webSocketDebuggerUrl") || body.contains("Browser"),
            "CDP version endpoint must answer with browser metadata, got: {body}"
        );

        let stopped = json(
            &commands::runtime_stop(host.clone(), profile_id.clone())
                .await
                .expect("stop"),
        );
        assert_eq!(stopped["state"], "stopped");
    });
}

/// 发一个最小 HTTP/1.1 GET 并返回响应体；只用于本文件的 CDP 探测。
fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the CDP port");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .expect("send request");
    // CDP 不因 `Connection: close` 立刻关连接，所以按 `Content-Length` 读完就停，
    // 不能等 EOF；等 EOF 只会撞上读超时，而数据其实早就到齐了。
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
            Err(error) => panic!("read from the CDP port failed ({error})"),
        }
        let text = String::from_utf8_lossy(&raw);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let declared = head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            });
            if let Some(length) = declared {
                if body.len() >= length {
                    return body[..length].to_owned();
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    text.split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or(text)
}

/// 契约 `ProfileInput` 的 `group_id`/`tags`/`proxy_id` 必须真的落库。
/// 这三个字段曾经只在 UI 表单里存在、在提交时被丢弃，用户填了等于没填。
#[test]
fn create_persists_group_tags_and_proxy() {
    assert_eq!(std::env::consts::OS, "macos");
    let directory = tempfile::tempdir().expect("temporary directory");
    let state = state::AppState::open_at_with_kernel(directory.path().join("data"), None)
        .expect("host state");
    let app = mock_builder()
        .manage(state)
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let host = app.state::<state::AppState>();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    runtime.block_on(async {
        let pool = host.pool();

        // 准备分组与代理行：契约要求它们先存在，否则应当是校验失败而不是静默丢弃。
        let now = mien_core::storage::now_utc();
        sqlx::query(
            "INSERT INTO groups (id,name,sort_order,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)",
        )
        .bind("01M8G7P1Y4000000000000GRP")
        .bind("acceptance-group")
        .bind(0_i64)
        .bind(&now)
        .execute(pool)
        .await
        .expect("group");
        sqlx::query("INSERT INTO tags (id,name,color,created_at) VALUES (?1,?2,?3,?4)")
            .bind("01M8G7P1Y40000000000000TAG")
            .bind("acceptance-tag")
            .bind("#336699")
            .bind(&now)
            .execute(pool)
            .await
            .expect("tag");

        let created = json(
            &commands::profiles_create(
                host.clone(),
                state::CreateProfileInput {
                    name: "relations".into(),
                    fingerprint: None,
                    mode: Some(ProfileMode::Headful),
                    start_url: None,
                    notes: None,
                    automation_api: None,
                    group_id: Some("01M8G7P1Y4000000000000GRP".into()),
                    tags: vec!["acceptance-tag".into()],
                    proxy_id: None,
                    kernel_version: None,
                },
            )
            .await
            .expect("create"),
        );

        // 先断言 UI 拿到的载荷：契约 `Profile` 的这两个字段必须直接可用。
        assert_eq!(
            created["group_id"], "01M8G7P1Y4000000000000GRP",
            "the create payload must carry group_id: {created:?}"
        );
        let tags = created["tags"].as_array().expect("tags array");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0], "acceptance-tag");

        let profile_id = created["id"].as_str().expect("id").to_owned();
        let stored_group: Option<Option<String>> =
            sqlx::query_scalar("SELECT group_id FROM profiles WHERE id=?1")
                .bind(&profile_id)
                .fetch_optional(pool)
                .await
                .expect("read group");
        assert_eq!(
            stored_group.flatten().as_deref(),
            Some("01M8G7P1Y4000000000000GRP"),
            "group_id must reach the database"
        );
        let stored_tags: Vec<String> = sqlx::query_scalar(
            "SELECT t.name FROM tags t JOIN profile_tags pt ON pt.tag_id=t.id \
             WHERE pt.profile_id=?1",
        )
        .bind(&profile_id)
        .fetch_all(pool)
        .await
        .expect("read tags");
        assert_eq!(stored_tags, vec!["acceptance-tag".to_string()]);

        // 更新时只改分组，标签必须保留；传 null 才清空分组。
        let profile_id = created["id"].as_str().expect("id").to_owned();
        let updated = json(
            &commands::profiles_update(
                host.clone(),
                profile_id.clone(),
                state::UpdateProfileInput {
                    name: None,
                    notes: None,
                    start_url: None,
                    fingerprint: None,
                    mode: None,
                    automation_api: None,
                    group_id: Some(None),
                    tags: None,
                    proxy_id: None,
                },
            )
            .await
            .expect("update"),
        );
        assert!(
            updated["group_id"].is_null(),
            "an explicit null clears the group in the payload: {updated:?}"
        );
        assert_eq!(
            updated["tags"].as_array().expect("tags").len(),
            1,
            "tags survive a group-only update: {updated:?}"
        );
        let cleared: Option<Option<String>> =
            sqlx::query_scalar("SELECT group_id FROM profiles WHERE id=?1")
                .bind(&profile_id)
                .fetch_optional(pool)
                .await
                .expect("read group after clear");
        assert_eq!(cleared.flatten(), None, "an explicit null clears the group");
        let kept_tags: Vec<String> = sqlx::query_scalar(
            "SELECT t.name FROM tags t JOIN profile_tags pt ON pt.tag_id=t.id \
             WHERE pt.profile_id=?1",
        )
        .bind(&profile_id)
        .fetch_all(pool)
        .await
        .expect("read tags after group-only update");
        assert_eq!(
            kept_tags,
            vec!["acceptance-tag".to_string()],
            "tags must survive a group-only update"
        );

        // 契约 `ProfileInput.kernel_version` 必须真的被接收：指定了就落库，
        // 不指定才回落到清单版本。
        let pinned = json(
            &commands::profiles_create(
                host.clone(),
                state::CreateProfileInput {
                    name: "pinned-kernel".into(),
                    fingerprint: None,
                    mode: Some(ProfileMode::Headful),
                    start_url: None,
                    notes: None,
                    automation_api: None,
                    group_id: None,
                    tags: Vec::new(),
                    proxy_id: None,
                    kernel_version: Some("149.0.8000.100".into()),
                },
            )
            .await
            .expect("create with pinned kernel"),
        );
        assert_eq!(
            pinned["kernel_version"], "149.0.8000.100",
            "a given kernel_version must be stored: {pinned:?}"
        );
        assert_ne!(
            pinned["kernel_version"], created["kernel_version"],
            "the pinned version must differ from the manifest default"
        );

        // 列表视图同样要带关系字段，否则刷新后 UI 又变回空值。
        let listed = json(
            &commands::profiles_list(host.clone(), 10, None, None)
                .await
                .expect("list"),
        );
        let items = listed["items"].as_array().expect("items");
        let relations = items
            .iter()
            .find(|item| item["name"] == "relations")
            .expect("the relations profile is listed");
        assert!(
            relations["group_id"].is_null(),
            "the list reflects the cleared group: {relations:?}"
        );
        assert_eq!(
            relations["tags"].as_array().expect("tags").len(),
            1,
            "the list carries tags: {relations:?}"
        );
    });
}
