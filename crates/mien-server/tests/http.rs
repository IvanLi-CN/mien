//! HTTP 行为验收：健康/就绪语义、版本发现、错误信封与安全头。
//!
//! `mien_core::storage::open_and_migrate` 自己创建运行时，因此测试不能在
//! `#[tokio::test]` 内调用它；这里与 `mien-core` 既有集成测试一致，用同步测试
//! 加一个显式当前线程运行时。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use mien_server::state::{AppState, Config, ConfigError, validate_bind};
use mien_server::{REQUEST_ID_HEADER, router};
use tempfile::TempDir;
use tower::ServiceExt;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// 迁移后的临时数据根、状态与池，供一次测试使用。
fn migrated_root() -> (TempDir, AppState, sqlx::SqlitePool) {
    let dir = TempDir::new().expect("temp dir");
    let pool = mien_core::storage::open_and_migrate(&dir.path().join("mien.db")).expect("migrate");
    let state = AppState::new(dir.path(), Some(pool.clone()));
    (dir, state, pool)
}

/// 没有任何依赖的宿主：数据根存在，数据库未打开。
fn state_without_database(dir: &std::path::Path) -> AppState {
    AppState::new(dir, None)
}

async fn call(app: axum::Router, method: &str, path: &str) -> Response {
    app.oneshot(
        Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .expect("request"),
    )
    .await
    .expect("response")
}

async fn json(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn assert_security_headers(response: &Response) {
    let headers = response.headers();
    let csp = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("content-security-policy")
        .to_str()
        .expect("ascii");
    assert!(csp.contains("default-src 'self'"), "csp: {csp}");
    assert!(
        !csp.contains("'unsafe-inline'"),
        "inline script and style need 'unsafe-inline'; must stay blocked: {csp}"
    );
    assert_eq!(
        headers
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .expect("x-content-type-options"),
        "nosniff"
    );
    assert_eq!(
        headers
            .get(header::REFERRER_POLICY)
            .expect("referrer-policy"),
        "no-referrer"
    );
    assert_eq!(
        headers
            .get(header::X_FRAME_OPTIONS)
            .expect("x-frame-options"),
        "DENY"
    );
    assert!(
        headers.contains_key(REQUEST_ID_HEADER),
        "x-request-id must be present"
    );
}

#[test]
fn healthz_is_200_without_any_dependency() {
    let dir = TempDir::new().expect("temp dir");
    let state = state_without_database(dir.path());
    assert!(state.db().is_none());

    let rt = runtime();
    let response = rt.block_on(call(router(state), "GET", "/healthz"));
    assert_eq!(response.status(), StatusCode::OK);
    assert_security_headers(&response);
    assert_eq!(
        rt.block_on(json(response)),
        serde_json::json!({"status": "ok"})
    );
}

#[test]
fn healthz_stays_200_after_the_database_file_disappears() {
    let (dir, state, pool) = migrated_root();
    drop(pool);
    std::fs::remove_file(dir.path().join("mien.db")).expect("remove database");

    let rt = runtime();
    let response = rt.block_on(call(router(state), "GET", "/healthz"));
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        rt.block_on(json(response)),
        serde_json::json!({"status": "ok"})
    );
}

#[test]
fn readyz_is_200_when_database_and_kernel_are_available() {
    let (_dir, state, _pool) = migrated_root();

    let rt = runtime();
    let response = rt.block_on(call(router(state), "GET", "/readyz"));
    assert_eq!(response.status(), StatusCode::OK);
    assert_security_headers(&response);

    let body = rt.block_on(json(response));
    assert_eq!(body["status"], "ok");
    assert_eq!(body["checks"]["database"]["ok"], true);
    assert_eq!(body["checks"]["kernel"]["ok"], true);
    let rendered = body.to_string();
    assert!(!rendered.contains("mien.db"), "no data paths: {rendered}");
    assert!(
        !rendered.contains("DISPLAY") && !rendered.contains("Xvfb"),
        "headless readiness must not require a display: {rendered}"
    );
}

#[test]
fn readyz_is_503_when_the_database_file_is_gone() {
    let (dir, state, pool) = migrated_root();
    drop(pool);
    std::fs::remove_file(dir.path().join("mien.db")).expect("remove database");

    let rt = runtime();
    let response = rt.block_on(call(router(state), "GET", "/readyz"));
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_security_headers(&response);

    let body = rt.block_on(json(response));
    assert_eq!(body["status"], "unavailable");
    assert_eq!(body["checks"]["database"]["ok"], false);
}

#[test]
fn readyz_is_503_without_an_open_database_but_reports_kernel_state() {
    let dir = TempDir::new().expect("temp dir");
    let state = state_without_database(dir.path());

    let rt = runtime();
    let response = rt.block_on(call(router(state), "GET", "/readyz"));
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = rt.block_on(json(response));
    assert_eq!(body["checks"]["database"]["ok"], false);
    assert_eq!(body["checks"]["kernel"]["ok"], true);
}

#[test]
fn discovery_reports_version_and_deprecation_window() {
    let (_dir, state, _pool) = migrated_root();

    let rt = runtime();
    let response = rt.block_on(call(router(state), "GET", "/api/v1"));
    assert_eq!(response.status(), StatusCode::OK);
    assert_security_headers(&response);

    let body = rt.block_on(json(response));
    assert_eq!(body["version"], "v1");
    assert_eq!(body["deprecation_window_days"], 90);
    assert_eq!(body["supported_versions"], serde_json::json!(["v1"]));
}

#[test]
fn unknown_api_version_is_404_with_the_error_envelope() {
    let (_dir, state, _pool) = migrated_root();

    let rt = runtime();
    let response = rt.block_on(call(router(state), "GET", "/api/v2"));
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_security_headers(&response);

    let header_id = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .expect("x-request-id")
        .to_str()
        .expect("ascii")
        .to_owned();

    let body = rt.block_on(json(response));
    assert_eq!(body["code"], "VALIDATION_FAILED");
    assert_eq!(body["requestId"], header_id);
    assert_eq!(body["request_id"], header_id);
    assert_eq!(body["details"]["requested_version"], "v2");
    assert!(
        mien_core::ErrorCode::parse(body["code"].as_str().expect("code string")).is_some(),
        "code must be a catalogued ErrorCode"
    );
}

#[test]
fn unknown_routes_use_the_same_envelope_and_headers() {
    let (_dir, state, _pool) = migrated_root();

    let rt = runtime();
    let response = rt.block_on(call(router(state), "GET", "/nope"));
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_security_headers(&response);

    let body = rt.block_on(json(response));
    assert_eq!(body["code"], "VALIDATION_FAILED");
    assert!(body["requestId"].as_str().is_some_and(|id| !id.is_empty()));
}

#[test]
fn non_loopback_bind_without_a_public_url_fails_startup_validation() {
    let bind = "0.0.0.0:8787".parse().expect("addr");
    assert_eq!(
        validate_bind(bind, None),
        Err(ConfigError::MissingPublicUrl { bind })
    );
    let error = Config::from_lookup(|key| match key {
        "MIEN_BIND" => Some("192.0.2.10:8787".to_owned()),
        _ => None,
    })
    .expect_err("startup validation must reject a public bind without MIEN_PUBLIC_URL");
    assert!(matches!(error, ConfigError::MissingPublicUrl { .. }));
}

#[test]
fn loopback_bind_needs_no_public_url() {
    let config = Config::from_lookup(|key| match key {
        "MIEN_BIND" => Some("127.0.0.1:8787".to_owned()),
        _ => None,
    })
    .expect("loopback development config");
    assert!(config.public_url.is_none());
}

/// `AppState::open` 产出的池可查询；`/readyz` 的失败来自注入条件而非坏 fixture。
#[test]
fn open_produces_a_queryable_database() {
    let dir = TempDir::new().expect("temp dir");
    let state = AppState::open(dir.path()).expect("open");
    let pool = state.db().expect("pool").clone();

    let rt = runtime();
    let value: i64 = rt
        .block_on(sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&pool))
        .expect("query");
    assert_eq!(value, 1);
    assert!(state.db_path().ends_with("mien.db"));
}

#[test]
fn migrations_are_applied_by_open() {
    let (_dir, _state, pool) = migrated_root();
    let rt = runtime();
    let version: Option<i64> = rt
        .block_on(
            sqlx::query_scalar::<_, Option<i64>>("SELECT max(version) FROM schema_migrations")
                .fetch_one(&pool),
        )
        .expect("version");
    assert!(version.unwrap_or(0) >= 1);
}
