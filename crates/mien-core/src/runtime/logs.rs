//! 运行时结构化日志：有界尾部、脱敏与轮转骨架。

use serde::Serialize;
use std::{collections::VecDeque, path::PathBuf, sync::Mutex};

const DEFAULT_MAX_LINES: usize = 1024;
const DEFAULT_MAX_BYTES: usize = 512 * 1024;
const FILE_LIMIT: u64 = 10 * 1024 * 1024;
const FILE_COUNT: usize = 5;

#[derive(Debug, Serialize)]
struct LogLine<'a> {
    ts: String,
    stream: &'a str,
    line: String,
}

#[derive(Debug)]
struct LogState {
    lines: VecDeque<String>,
    bytes: usize,
}

/// 每档案日志的内存尾部与 JSONL 文件写入器。
pub struct RuntimeLogs {
    dir: PathBuf,
    max_lines: usize,
    max_bytes: usize,
    state: Mutex<LogState>,
}

impl RuntimeLogs {
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        Self::with_limits(dir, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES)
    }

    pub fn with_limits(
        dir: impl Into<PathBuf>,
        max_lines: usize,
        max_bytes: usize,
    ) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            max_lines: max_lines.max(1),
            max_bytes: max_bytes.max(128),
            state: Mutex::new(LogState {
                lines: VecDeque::new(),
                bytes: 0,
            }),
        })
    }

    pub fn record(&self, stream: &str, line: &str) {
        let redacted = redact(line);
        let json = serde_json::to_string(&LogLine {
            ts: crate::storage::now_utc(),
            stream,
            line: redacted,
        })
        .unwrap_or_else(|_| "{\"line\":\"[redacted]\"}".into());
        let mut state = self.state.lock().expect("log lock");
        state.bytes += json.len();
        state.lines.push_back(json.clone());
        while state.lines.len() > self.max_lines || state.bytes > self.max_bytes {
            if let Some(old) = state.lines.pop_front() {
                state.bytes = state.bytes.saturating_sub(old.len());
            }
        }
        drop(state);
        let _ = self.append_rotated(&json);
    }

    pub fn tail(&self, limit: usize) -> Vec<String> {
        let state = self.state.lock().expect("log lock");
        state
            .lines
            .iter()
            .rev()
            .take(limit.min(1000))
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    fn append_rotated(&self, line: &str) -> std::io::Result<()> {
        use std::io::Write;
        let path = self.dir.join("runtime.jsonl");
        if path.exists() && std::fs::metadata(&path)?.len() >= FILE_LIMIT {
            for index in (1..FILE_COUNT).rev() {
                let old = self.dir.join(format!("runtime.jsonl.{index}"));
                let new = self.dir.join(format!("runtime.jsonl.{}", index + 1));
                if old.exists() {
                    let _ = std::fs::rename(old, new);
                }
            }
            let _ = std::fs::rename(&path, self.dir.join("runtime.jsonl.1"));
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{line}")
    }
}

/// 脱敏凭据、Cookie、授权头与完整 URL。
pub fn redact(input: &str) -> String {
    let mut output = input.to_owned();
    for key in [
        "password",
        "passwd",
        "token",
        "cookie",
        "authorization",
        "proxy_url",
        "share_url",
    ] {
        output = redact_key(&output, key);
    }
    let mut sanitized = String::with_capacity(output.len());
    let mut rest = output.as_str();
    while let Some(start) = rest.find("http://").or_else(|| rest.find("https://")) {
        sanitized.push_str(&rest[..start]);
        let end = rest[start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |index| start + index);
        sanitized.push_str("[redacted-url]");
        rest = &rest[end..];
    }
    sanitized.push_str(rest);
    sanitized
}

fn redact_key(input: &str, key: &str) -> String {
    let mut output = input.to_owned();
    for separator in [format!("{key}="), format!("\"{key}\":")] {
        let lower = output.to_ascii_lowercase();
        let Some(offset) = lower.find(&separator) else {
            continue;
        };
        let value_start = offset + separator.len();
        let value_end = output[value_start..]
            .find(char::is_whitespace)
            .map_or(output.len(), |index| value_start + index);
        output.replace_range(value_start..value_end, "[redacted]");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_secrets_and_urls() {
        let text = redact("token=secret Cookie=abc https://user:pass@example.invalid/path");
        assert!(!text.contains("secret"));
        assert!(!text.contains("example.invalid"));
    }
    #[test]
    fn tail_is_bounded() {
        let dir = std::env::temp_dir().join(format!("mien-log-{}", std::process::id()));
        let logs = RuntimeLogs::with_limits(&dir, 2, 256).unwrap();
        logs.record("stderr", "a");
        logs.record("stderr", "b");
        logs.record("stderr", "c");
        assert_eq!(logs.tail(100).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
