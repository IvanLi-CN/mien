//! 同源静态 Web UI 托管（`REQ-API-004`、`REQ-API-015`）。
//!
//! 静态资源与 `/api/v1` 由同一个 axum Router 提供，因此请求标识与安全头层同样
//! 覆盖静态响应。目录只在启动时解析一次：目录缺失只意味着没有 UI，`/api/*` 与
//! 健康检查不受影响，服务端不会因此 panic 或拒绝启动。
//!
//! 判定顺序固定为 `/api/*`、`/healthz`、`/readyz` → 静态文件 → SPA 入口；
//! 未知 API 路径始终得到 JSON 404，绝不会被 `index.html` 吞掉。

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::RequestId;
use crate::state::AppState;

/// 静态资源根目录环境变量。
pub const WEB_DIST_ENV: &str = "MIEN_WEB_DIST";

/// `index.html` 与普通文件都必须先与服务器确认；只有带哈希的 `assets/*` 可长期缓存。
const NO_CACHE: &str = "no-cache";
/// Vite 产物文件名带内容哈希，内容变化即换名。
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// 启动时定位到的静态资源根。
///
/// 路径已规范化，请求期只做 `root.join(relative)` 与最终前缀校验，不再探测候选目录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebRoot {
    root: PathBuf,
}

impl WebRoot {
    /// 显式目录。目录不存在或不可规范化时返回 `None`：UI 关闭，启动继续。
    pub fn new(path: impl Into<PathBuf>) -> Option<Self> {
        let path = path.into();
        match canonical_dir(&path) {
            Some(root) => {
                tracing::info!(path = %root.display(), "serving the web UI from disk");
                Some(Self { root })
            }
            None => {
                tracing::warn!(
                    path = %path.display(),
                    "web assets are missing; the UI is not served"
                );
                None
            }
        }
    }

    /// 默认顺序：`MIEN_WEB_DIST`，其次 `<cwd>/dist`，最后可执行文件旁的 `web/`。
    ///
    /// `MIEN_WEB_DIST` 一旦设置就独占：值不可用时关闭 UI，而不是静默换用别的构建。
    pub fn resolve() -> Option<Self> {
        let configured = std::env::var_os(WEB_DIST_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let cwd = std::env::current_dir().ok();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        Self::resolve_with(configured, cwd.as_deref(), exe_dir.as_deref())
    }

    /// [`WebRoot::resolve`] 的纯参数版本：显式配置 → `<cwd>/dist` → `<exe-dir>/web`。
    fn resolve_with(
        configured: Option<PathBuf>,
        cwd: Option<&Path>,
        exe_dir: Option<&Path>,
    ) -> Option<Self> {
        if let Some(path) = configured {
            return Self::new(path);
        }
        for candidate in cwd
            .map(|cwd| cwd.join("dist"))
            .into_iter()
            .chain(exe_dir.map(|dir| dir.join("web")))
        {
            if let Some(root) = canonical_dir(&candidate) {
                tracing::info!(path = %root.display(), "serving the web UI from disk");
                return Some(Self { root });
            }
        }
        tracing::info!("no web assets directory found; the UI is not served");
        None
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// 规范化一个存在的目录；不存在或不是目录时返回 `None`。
fn canonical_dir(path: &Path) -> Option<PathBuf> {
    let root = path.canonicalize().ok()?;
    root.is_dir().then_some(root)
}

/// Router 兜底：先保留 API 面，再尝试静态文件，最后按 `Accept` 决定是否回退到 SPA。
pub async fn fallback(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    request: Request,
) -> Response {
    let path = request.uri().path().to_owned();
    // `/api/*`、`/healthz`、`/readyz` 永远不进入静态查找，未知路径仍是 JSON 404。
    // 编码形态（`/api%2fv1/...`）解码后同样按 API 面处理，不能借编码避开隔离。
    let encoded = relative_path(&path);
    let encoded_is_api = encoded
        .as_deref()
        .is_some_and(|relative| relative == "api" || relative.starts_with("api/"));
    if crate::is_api_surface(&path) || encoded_is_api || request.method() != Method::GET {
        return crate::not_found_response(&request_id, &path);
    }

    let Some(root) = state.web_dist() else {
        return crate::not_found_response(&request_id, &path);
    };
    let root = root.root();
    // 非法路径直接 404：即使浏览器带 `Accept: text/html` 也不得回退到 index。
    let Some(relative) = encoded else {
        return crate::not_found_response(&request_id, &path);
    };
    let requested = if relative.is_empty() {
        "index.html"
    } else {
        relative.as_str()
    };
    if let Some(response) = serve_file(root, requested).await {
        return response;
    }
    // 缺资源不能伪装成页面：`assets/*` 与带扩展名的路径只回真实 404。
    if is_navigation(&relative) && accepts_html(request.headers()) {
        if let Some(response) = serve_file(root, "index.html").await {
            return response;
        }
    }
    crate::not_found_response(&request_id, &path)
}

/// 只有前端路由（无扩展名、不在 `assets/` 下）才能回退到 `index.html`。
fn is_navigation(relative: &str) -> bool {
    let file = relative.rsplit('/').next().unwrap_or(relative);
    !relative.starts_with("assets/") && !file.contains('.')
}

/// 解码请求路径并拆成安全的相对路径。
///
/// 拒绝解码失败、反斜杠、NUL、绝对路径、空段、`.` 与 `..`；其余段只可能拼出根目录
/// 之内的路径。是否越过根目录仍由 [`serve_file`] 的规范化前后缀校验兜底。
fn relative_path(path: &str) -> Option<String> {
    let decoded = percent_decode(path)?;
    if decoded.contains('\\') || decoded.contains('\0') {
        return None;
    }
    // 只允许恰好一个前导 `/`；`//etc/passwd` 这类绝对路径形态与空段一并拒绝。
    let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
    if trimmed.is_empty() {
        return Some(String::new());
    }
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        segments.push(segment);
    }
    Some(segments.join("/"))
}

/// 单次百分号解码；非法转义返回 `None`。
///
/// 只解码一次：`%252e%252e` 解码为字面量 `%2e%2e`，不会再次解释为 `..`。
fn percent_decode(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 导航请求（浏览器地址栏、刷新、直达深链接）带 `Accept: text/html`。
fn accepts_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|item| {
                item.split(';')
                    .next()
                    .is_some_and(|media| media.trim().eq_ignore_ascii_case("text/html"))
            })
        })
}

/// 读取并响应一个文件；不存在、不是文件或逃出根目录时返回 `None`。
async fn serve_file(root: &Path, relative: &str) -> Option<Response> {
    let resolved = tokio::fs::canonicalize(root.join(relative)).await.ok()?;
    // 规范化后仍必须位于根目录内：符号链接也不能把读取带出静态根。
    if !resolved.starts_with(root) {
        return None;
    }
    // 目录或不可读路径在这里读失败，与不存在同样按 404 处理。
    let bytes = tokio::fs::read(&resolved).await.ok()?;
    let mut response = (StatusCode::OK, Body::from(bytes)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(relative)),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control(relative)),
    );
    Some(response)
}

/// `assets/*` 是 Vite 的哈希产物；`index.html` 与其余文件每次都要重新校验。
fn cache_control(relative: &str) -> &'static str {
    if relative.starts_with("assets/") {
        IMMUTABLE
    } else {
        NO_CACHE
    }
}

/// 扩展名到 `Content-Type`；未知扩展名回退到 `application/octet-stream`。
fn content_type(relative: &str) -> &'static str {
    let file = relative.rsplit('/').next().unwrap_or(relative);
    let extension = file.rsplit_once('.').map_or("", |(_, extension)| extension);
    CONTENT_TYPES
        .iter()
        .find(|(known, _)| extension.eq_ignore_ascii_case(known))
        .map_or("application/octet-stream", |(_, value)| *value)
}

/// 静态产物可能出现的扩展名；顺序无关，匹配忽略大小写。
const CONTENT_TYPES: &[(&str, &str)] = &[
    ("html", "text/html; charset=utf-8"),
    ("htm", "text/html; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("json", "application/json"),
    ("map", "application/json"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("ico", "image/x-icon"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
    ("wasm", "application/wasm"),
    ("txt", "text/plain; charset=utf-8"),
    ("webmanifest", "application/manifest+json"),
    ("xml", "application/xml"),
    ("pdf", "application/pdf"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_reject_traversal_and_absolute_forms() {
        assert_eq!(relative_path("/").as_deref(), Some(""));
        assert_eq!(
            relative_path("/assets/x-abc.js").as_deref(),
            Some("assets/x-abc.js")
        );
        assert_eq!(relative_path("/../Cargo.toml"), None);
        assert_eq!(relative_path("/assets/%2e%2e/Cargo.toml"), None);
        assert_eq!(relative_path("/%2E%2E/README.md"), None);
        assert_eq!(relative_path("/..%5CCargo.toml"), None);
        assert_eq!(relative_path("/assets/%zz/x.js"), None);
        assert_eq!(relative_path("//assets//x.js"), None);
        assert_eq!(relative_path("//etc/passwd"), None);
        assert_eq!(relative_path("/assets/%2f%2f..%2fCargo.toml"), None);
        assert_eq!(
            relative_path("/api%2fv1/nope").as_deref(),
            Some("api/v1/nope")
        );
        // 双重编码只是字面量，不会再被解释为 `..`。
        assert_eq!(
            relative_path("/%252e%252e/Cargo.toml").as_deref(),
            Some("%2e%2e/Cargo.toml")
        );
    }

    #[test]
    fn content_types_cover_the_vite_output_set() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type("assets/x-abc.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type("assets/x-abc.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("assets/logo.svg"), "image/svg+xml");
        assert_eq!(content_type("assets/data.JSON"), "application/json");
        assert_eq!(content_type("assets/font.woff2"), "font/woff2");
        assert_eq!(content_type("assets/blob.bin"), "application/octet-stream");
        assert_eq!(
            content_type("assets/no-extension"),
            "application/octet-stream"
        );
    }

    #[test]
    fn only_hashed_assets_are_immutable() {
        assert_eq!(
            cache_control("assets/index-abc123.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_control("index.html"), "no-cache");
        assert_eq!(cache_control("assets"), "no-cache");
        assert_eq!(cache_control("manifest.webmanifest"), "no-cache");
    }

    #[test]
    fn accept_html_requires_the_media_type() {
        let mut headers = HeaderMap::new();
        assert!(!accepts_html(&headers));
        headers.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
        assert!(!accepts_html(&headers));
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/html,application/xhtml+xml;q=0.9"),
        );
        assert!(accepts_html(&headers));
        headers.insert(header::ACCEPT, HeaderValue::from_static("TEXT/HTML"));
        assert!(accepts_html(&headers));
    }

    #[test]
    fn only_extensionless_routes_may_fall_back_to_the_spa_shell() {
        assert!(is_navigation("profiles/abc"));
        assert!(is_navigation(""));
        assert!(!is_navigation("assets/missing-abc.js"));
        assert!(!is_navigation("sw.js"));
        assert!(!is_navigation("favicon.ico"));
    }

    #[test]
    fn dist_resolution_prefers_the_environment_then_cwd_then_the_executable_dir() {
        let cwd = tempfile::TempDir::new().expect("cwd");
        let exe = tempfile::TempDir::new().expect("exe");
        let configured = tempfile::TempDir::new().expect("configured");

        // 都没有时返回 `None`，不是错误。
        assert_eq!(
            WebRoot::resolve_with(None, Some(cwd.path()), Some(exe.path())),
            None
        );

        // `<exe-dir>/web` 先于缺失的 `<cwd>/dist` 生效。
        std::fs::create_dir(exe.path().join("web")).expect("exe web");
        assert_eq!(
            WebRoot::resolve_with(None, Some(cwd.path()), Some(exe.path()))
                .as_ref()
                .map(WebRoot::root),
            Some(
                exe.path()
                    .join("web")
                    .canonicalize()
                    .expect("canonical")
                    .as_path()
            )
        );

        // `<cwd>/dist` 优先级更高（Vite 的 `outDir`）。
        std::fs::create_dir(cwd.path().join("dist")).expect("cwd dist");
        assert_eq!(
            WebRoot::resolve_with(None, Some(cwd.path()), Some(exe.path()))
                .as_ref()
                .map(WebRoot::root),
            Some(
                cwd.path()
                    .join("dist")
                    .canonicalize()
                    .expect("canonical")
                    .as_path()
            )
        );

        // `MIEN_WEB_DIST` 独占，不会因它可用而回落到默认目录。
        assert_eq!(
            WebRoot::resolve_with(
                Some(configured.path().to_path_buf()),
                Some(cwd.path()),
                Some(exe.path())
            )
            .as_ref()
            .map(WebRoot::root),
            Some(
                configured
                    .path()
                    .canonicalize()
                    .expect("canonical")
                    .as_path()
            )
        );
        // 显式值不可用时关闭 UI，而不是静默换用别的构建。
        assert_eq!(
            WebRoot::resolve_with(
                Some(cwd.path().join("missing")),
                Some(cwd.path()),
                Some(exe.path())
            ),
            None
        );
    }
}
