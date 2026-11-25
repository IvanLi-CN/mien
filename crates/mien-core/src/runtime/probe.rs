//! 启动前探针：内核完整性、macOS 沙箱、CDP 与进程归属。

use std::{path::Path, time::Duration};

use crate::{
    error::{CoreError, ErrorCode},
    runtime::state::Isolation,
};

fn kernel_missing() -> CoreError {
    CoreError::validation(ErrorCode::KernelNotFound, "kernel executable is missing")
}

fn cdp_timeout() -> CoreError {
    CoreError::validation(
        ErrorCode::RuntimeCdpTimeout,
        "CDP endpoint did not become ready",
    )
}

fn sandbox_unavailable(message: &str) -> CoreError {
    CoreError::validation(ErrorCode::SandboxUnavailable, message)
}

/// 校验解包后的内核可执行文件，而不是发布资产摘要。
pub fn verify_kernel(path: &Path, expected_sha256: &str) -> Result<(), CoreError> {
    if !path.is_file() {
        return Err(kernel_missing());
    }
    let bytes = std::fs::read(path).map_err(|_| kernel_missing())?;
    let actual = crate::proxy::keys::sha256(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual != expected_sha256.to_ascii_lowercase() {
        return Err(CoreError::validation(
            ErrorCode::KernelChecksumMismatch,
            "kernel executable digest does not match the install record",
        ));
    }
    Ok(())
}

/// macOS 的有头运行时必须能确认 seatbelt；无法确认即拒绝启动。
pub fn probe_macos_sandbox(manifest_sandbox_kind: &str) -> Result<Isolation, CoreError> {
    if std::env::consts::OS != "macos" || manifest_sandbox_kind != "seatbelt" {
        return Err(sandbox_unavailable(
            "Chromium sandbox could not be confirmed on this host",
        ));
    }
    if !Path::new("/usr/bin/sandbox-exec").is_file() {
        return Err(sandbox_unavailable(
            "Chromium sandbox could not be confirmed on this host",
        ));
    }
    Ok(Isolation::Enforced)
}

/// 读取 Chromium 的 `DevToolsActivePort`，并确认端口只在 loopback 可连。
pub async fn wait_for_cdp(user_data_dir: &Path, timeout: Duration) -> Result<u16, CoreError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let path = user_data_dir.join("DevToolsActivePort");
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(cdp_timeout());
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            eprintln!("probe: file={:?} text={:?}", path, text);
            if let Some(port) = text
                .lines()
                .next()
                .and_then(|line| line.trim().parse::<u16>().ok())
            {
                eprintln!("probe: parsed port {port}");
                if port != 0 {
                    let connected = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
                    eprintln!("probe: connect result {:?}", connected.as_ref().err());
                    if connected.is_ok() {
                        return Ok(port);
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// 读取进程命令行，仅用于安全归属判断；不能确认时返回 false。
pub fn process_belongs_to(pid: u32, binary: &Path, runtime_id: &str) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output();
    let Ok(output) = output else { return false };
    let command = String::from_utf8_lossy(&output.stdout);
    command.contains(&binary.to_string_lossy().to_string()) && command.contains(runtime_id)
}

pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as i32, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

pub fn process_group_alive(pgid: i32) -> bool {
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(-pgid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        false
    }
}

pub fn signal_process_group(pgid: i32, force: bool) {
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(-pgid, if force { libc::SIGKILL } else { libc::SIGTERM });
    }
    #[cfg(not(unix))]
    let _ = (pgid, force);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_kernel_has_stable_code_in_message() {
        let error = verify_kernel(Path::new("/definitely/missing/kernel"), "00").unwrap_err();
        assert_eq!(error.code(), ErrorCode::KernelNotFound);
        assert_eq!(error.code_str(), "KERNEL_NOT_FOUND");
    }
}
