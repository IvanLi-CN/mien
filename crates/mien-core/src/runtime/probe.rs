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

/// Linux: confirm Chromium's namespace sandbox on the running tree.
///
/// The browser process stays in the container namespaces; sandboxed zygotes and
/// renderers live in a private user and pid namespace with a seccomp filter. One
/// such descendant is the evidence. Any failure to read `/proc` is `Unknown`,
/// which callers must treat as fail-closed.
#[cfg(target_os = "linux")]
pub fn probe_linux_isolation(root_pid: u32) -> Isolation {
    let (Some(init_user), Some(init_pid)) = (ns_link(1, "user"), ns_link(1, "pid")) else {
        return Isolation::Unknown;
    };
    for pid in descendants(root_pid) {
        let (Some(user), Some(pid_ns)) = (ns_link(pid, "user"), ns_link(pid, "pid")) else {
            continue;
        };
        if (user != init_user || pid_ns != init_pid) && seccomp_mode(pid) == Some(2) {
            return Isolation::Enforced;
        }
    }
    Isolation::Unsandboxed
}

#[cfg(target_os = "linux")]
fn ns_link(pid: u32, name: &str) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/ns/{name}"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(target_os = "linux")]
fn seccomp_mode(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Seccomp:"))
        .and_then(|value| value.trim().parse().ok())
}

/// Field 4 of `/proc/<pid>/stat`; the command name may contain spaces and
/// parentheses, so parse after the last `)`.
#[cfg(target_os = "linux")]
fn parse_ppid(stat: &str) -> Option<u32> {
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_whitespace();
    let _state = fields.next();
    fields.next()?.parse().ok()
}

#[cfg(target_os = "linux")]
fn descendants(root_pid: u32) -> Vec<u32> {
    let mut parents = std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        if let Some(ppid) = parse_ppid(&stat) {
            parents.insert(pid, ppid);
        }
    }
    let mut result = Vec::new();
    let mut frontier = vec![root_pid];
    while let Some(current) = frontier.pop() {
        for (pid, ppid) in &parents {
            if *ppid == current && *pid != current {
                result.push(*pid);
                frontier.push(*pid);
            }
        }
    }
    result
}

/// Linux fail-closed gate: poll briefly for sandboxed descendants, then refuse.
#[cfg(target_os = "linux")]
pub async fn require_linux_isolation(root_pid: u32) -> Result<Isolation, CoreError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match probe_linux_isolation(root_pid) {
            Isolation::Enforced => return Ok(Isolation::Enforced),
            Isolation::Unknown => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(sandbox_unavailable(
                        "Chromium sandbox state could not be read from /proc",
                    ));
                }
            }
            Isolation::Unsandboxed => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(sandbox_unavailable(
                        "Chromium namespace sandbox is not active",
                    ));
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_ppid_around_spaces_and_parentheses() {
        assert_eq!(parse_ppid("42 (a (b) c) S 7 42"), Some(7));
        assert_eq!(parse_ppid("1 (init) S 0 1"), Some(0));
        assert_eq!(parse_ppid("bad"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn self_probe_reads_proc() {
        let isolation = probe_linux_isolation(std::process::id());
        assert_ne!(isolation, crate::runtime::state::Isolation::Unknown);
    }

    #[test]
    fn missing_kernel_has_stable_code_in_message() {
        let error = verify_kernel(Path::new("/definitely/missing/kernel"), "00").unwrap_err();
        assert_eq!(error.code(), ErrorCode::KernelNotFound);
        assert_eq!(error.code_str(), "KERNEL_NOT_FOUND");
    }
}
