//! 服务端入口：解析配置、打开迁移后的数据库、启动 HTTP 服务。
//!
//! `mien_core::storage::open_and_migrate` 自己创建运行时（同步 API），因此存储
//! 初始化必须在进入异步运行时之前完成；在运行时内调用会触发嵌套运行时 panic。

use std::process::ExitCode;

use mien_server::serve;
use mien_server::state::{AppState, Config};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(error = %error, "invalid configuration");
            return ExitCode::FAILURE;
        }
    };

    let state = match AppState::open(&config.data_root) {
        Ok(state) => state,
        Err(error) => {
            tracing::error!(
                code = error.code_str(),
                detail = %error,
                data_root = %config.data_root.display(),
                "storage is unusable"
            );
            return ExitCode::FAILURE;
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(error = %error, "cannot create the async runtime");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(config.bind).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!(bind = %config.bind, error = %error, "cannot listen");
                return ExitCode::FAILURE;
            }
        };

        tracing::info!(
            bind = %config.bind,
            public_url = config.public_url.as_deref().unwrap_or("none"),
            data_root = %config.data_root.display(),
            "mien-server listening"
        );

        match serve(listener, state).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(error = %error, "server stopped with an error");
                ExitCode::FAILURE
            }
        }
    })
}
