use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const APP_ID: &str = "io.github.clash-verge-rev.clash-verge-rev";

/// Linux's core may use a portable/custom data directory. Read only process arguments.
#[cfg(target_os = "linux")]
fn running_dirs() -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            let args: Vec<_> = bytes
                .split(|b| *b == 0)
                .map(String::from_utf8_lossy)
                .collect();
            let Some(exe) = args.first() else {
                continue;
            };
            if !Path::new(exe.as_ref())
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("verge-mihomo"))
            {
                continue;
            }
            for pair in args.windows(2) {
                if pair[0] == "-d" {
                    result.push(PathBuf::from(pair[1].as_ref()));
                }
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

pub fn data_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_owned());
    }
    let default = dirs::data_dir()
        .context("无法定位 Clash Verge 数据目录，请使用 --data-dir")?
        .join(APP_ID);
    // Prefer this user's standard directory; don't select another user's running core.
    let mut candidates = vec![default.clone()];
    #[cfg(target_os = "linux")]
    if let Some(home) = dirs::home_dir() {
        candidates.extend(running_dirs().into_iter().filter(|p| p.starts_with(&home)));
    }
    if let Some(config) = dirs::config_dir() {
        candidates.push(config.join(APP_ID));
    }
    Ok(candidates
        .into_iter()
        .find(|p| p.join("profiles.yaml").is_file())
        .unwrap_or(default))
}

#[derive(Default, Deserialize)]
pub struct Controller {
    #[serde(rename = "external-controller")]
    pub address: Option<String>,
    #[serde(rename = "external-controller-unix")]
    pub socket: Option<PathBuf>,
    pub secret: Option<String>,
}

pub fn controller(dir: &Path) -> Result<Controller> {
    // Generated running configuration has precedence over the editable base settings.
    for name in ["clash-verge.yaml", "config.yaml"] {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                return serde_yaml::from_str(&text)
                    .with_context(|| format!("无法解析控制器配置 {}", path.display()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("无法读取 {}", path.display()));
            }
        }
    }
    Ok(Controller::default())
}

pub fn socket_candidates(dir: &Path, configured: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = configured {
        paths.push(if path.is_absolute() {
            path.to_owned()
        } else {
            dir.join(path)
        });
    }
    // Linux Rev uses the application data directory for its sidecar socket.
    paths.push(dir.join("verge-mihomo.sock"));
    paths.push(std::env::temp_dir().join("verge-mihomo.sock"));
    paths.push(PathBuf::from("/tmp/verge/verge-mihomo.sock"));
    paths
}

pub fn usable_socket(path: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(path).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

pub fn controller_url(address: Option<&str>) -> String {
    let address = address
        .filter(|s| !s.is_empty())
        .unwrap_or("127.0.0.1:9090");
    let address = address
        .strip_prefix("0.0.0.0:")
        .map(|port| format!("127.0.0.1:{port}"))
        .or_else(|| {
            address
                .strip_prefix("[::]:")
                .map(|port| format!("[::1]:{port}"))
        })
        .or_else(|| {
            address
                .strip_prefix(':')
                .map(|port| format!("127.0.0.1:{port}"))
        })
        .unwrap_or_else(|| address.to_owned());
    if address.contains("://") {
        address
    } else {
        format!("http://{address}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_runtime_controller_and_normalizes_wildcards() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.yaml"),
            "external-controller: 127.0.0.1:1111\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("clash-verge.yaml"), "external-controller: 0.0.0.0:9097\nsecret: test-secret\nexternal-controller-unix: verge-mihomo.sock\n").unwrap();
        let config = controller(dir.path()).unwrap();
        assert_eq!(
            controller_url(config.address.as_deref()),
            "http://127.0.0.1:9097"
        );
        assert_eq!(controller_url(Some("[::]:9097")), "http://[::1]:9097");
        assert_eq!(config.secret.as_deref(), Some("test-secret"));
        assert_eq!(
            socket_candidates(dir.path(), config.socket.as_deref())[0],
            dir.path().join("verge-mihomo.sock")
        );
    }
    #[cfg(unix)]
    #[test]
    fn finds_data_directory_socket_and_ignores_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("verge-mihomo.sock");
        let socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert!(usable_socket(&path));
        drop(socket);
        assert!(!usable_socket(&path));
    }
}
