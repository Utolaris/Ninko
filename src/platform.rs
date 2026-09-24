use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "linux")]
use std::process::Stdio;

const APP_ID: &str = "io.github.clash-verge-rev.clash-verge-rev";

/// Linux's core may use a portable/custom data directory. Read only process arguments.
#[cfg(target_os = "linux")]
fn running_dirs() -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if !owned_process(&entry.path()) {
                continue;
            }
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
                    let path = PathBuf::from(pair[1].as_ref());
                    if path.is_absolute() {
                        result.push(path);
                    } else if let Ok(cwd) = std::fs::read_link(entry.path().join("cwd")) {
                        result.push(cwd.join(path));
                    }
                }
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

/// The running mihomo core's actual controller endpoint can differ from the
/// controller settings stored in Clash Verge's editable per-user config.
pub struct RunningCore {
    pub socket: PathBuf,
    pub config: Option<PathBuf>,
}

fn core_from_args(args: &[String], cwd: Option<&Path>) -> Option<RunningCore> {
    let value_after = |names: &[&str]| {
        args.windows(2)
            .find(|pair| names.contains(&pair[0].as_str()))
            .map(|pair| pair[1].as_str())
    };
    let socket = value_after(&["-ext-ctl-unix", "--ext-ctl-unix"])?;
    let mut socket = PathBuf::from(socket);
    if !socket.is_absolute() {
        socket = cwd?.join(socket);
    }
    let config = match value_after(&["-f", "--config"]) {
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                Some(path)
            } else {
                cwd.map(|cwd| cwd.join(path))
            }
        }
        None => None,
    };
    Some(RunningCore { socket, config })
}

/// Read the current user's running core arguments without stopping or changing it.
pub fn running_cores() -> Vec<RunningCore> {
    #[cfg(target_os = "linux")]
    {
        let mut result = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Ok(bytes) = std::fs::read(entry.path().join("cmdline")) else {
                    continue;
                };
                let args: Vec<String> = bytes
                    .split(|b| *b == 0)
                    .filter(|arg| !arg.is_empty())
                    .map(|arg| String::from_utf8_lossy(arg).into_owned())
                    .collect();
                let Some(exe) = args.first() else {
                    continue;
                };
                if !Path::new(exe)
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("verge-mihomo"))
                {
                    continue;
                }
                if !core_process_owned(&entry.path(), &args) {
                    continue;
                }
                let cwd = std::fs::read_link(entry.path().join("cwd")).ok();
                if let Some(core) = core_from_args(&args, cwd.as_deref()) {
                    result.push(core);
                }
            }
        }
        result.sort_by(|a, b| a.socket.cmp(&b.socket));
        result
    }
    #[cfg(target_os = "macos")]
    {
        let mut result = Vec::new();
        if let Ok(output) = Command::new("/bin/ps")
            .args(["-ww", "-axo", "uid=,command="])
            .output()
        {
            let uid = unsafe { libc::geteuid() };
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let Some((owner, command)) = line.trim().split_once(char::is_whitespace) else {
                    continue;
                };
                let Ok(owner) = owner.parse::<u32>() else {
                    continue;
                };
                let command = command.trim_start();
                let Some(executable_end) = command
                    .find("verge-mihomo")
                    .map(|index| index + "verge-mihomo".len())
                else {
                    continue;
                };
                let executable = &command[..executable_end];
                if !Path::new(executable)
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("verge-mihomo"))
                {
                    continue;
                }
                // Service mode runs the core as root, but embeds the owner's UID
                // in its per-user runtime and socket paths.
                if owner != uid && !(owner == 0 && command.contains(&format!("/users/{uid}/"))) {
                    continue;
                }
                // ps flattens argv. The socket path has no spaces; config paths
                // may, so recover -f through the YAML filename before parsing.
                let mut args = vec![executable.to_owned()];
                if let Some(socket) = ps_value_after(command, "-ext-ctl-unix") {
                    args.extend(["-ext-ctl-unix".to_owned(), socket]);
                }
                if let Some(config) = ps_yaml_after(command, "-f") {
                    args.extend(["-f".to_owned(), config]);
                }
                if let Some(core) = core_from_args(&args, None) {
                    result.push(core);
                }
            }
        }
        result
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn ps_value_after(command: &str, flag: &str) -> Option<String> {
    let start = command.find(&format!("{flag} "))? + flag.len() + 1;
    command[start..]
        .split_whitespace()
        .next()
        .map(str::to_owned)
}

#[cfg(target_os = "macos")]
fn ps_yaml_after(command: &str, flag: &str) -> Option<String> {
    let start = command.find(&format!("{flag} "))? + flag.len() + 1;
    let value = &command[start..];
    let end = [".yaml", ".yml"]
        .iter()
        .filter_map(|suffix| value.find(suffix).map(|index| index + suffix.len()))
        .min()?;
    Some(value[..end].to_owned())
}

#[cfg(target_os = "linux")]
fn core_process_owned(path: &Path, args: &[String]) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(owner) = path.metadata().ok().map(|metadata| metadata.uid()) else {
        return false;
    };
    let uid = unsafe { libc::getuid() };
    owner == uid
        || (owner == 0
            && args
                .iter()
                .any(|arg| arg.contains(&format!("/users/{uid}/"))))
}

pub fn data_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        ensure!(
            path.is_dir(),
            "数据目录不存在：{}（请检查 --data-dir / CLASH_DATA_DIR）",
            path.display()
        );
        return Ok(path.to_owned());
    }
    let default = dirs::data_dir()
        .context("无法定位系统数据目录，请使用 --data-dir")?
        .join(APP_ID);
    // Prefer this user's standard directory; don't select another user's running core.
    // Official Clash Verge Rev uses dirs::data_dir()/APP_ID on both macOS and Linux.
    let mut candidates = vec![default.clone()];
    #[cfg(target_os = "linux")]
    candidates.extend(running_dirs());
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
                return parse_controller(&text, &path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("无法读取 {}", path.display()));
            }
        }
    }
    // No Verge-generated controller config: fall back to mihomo defaults later.
    Ok(Controller::default())
}

pub fn controller_file(path: &Path) -> Result<Controller> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取控制器配置 {}", path.display()))?;
    parse_controller(&text, path)
}

fn parse_controller(text: &str, path: &Path) -> Result<Controller> {
    serde_yaml::from_str(text).with_context(|| format!("无法解析控制器配置 {}", path.display()))
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
    // Linux Rev: sidecar socket lives in the application data directory.
    paths.push(dir.join("verge-mihomo.sock"));
    // macOS Rev: sidecar socket is in the per-user temp dir (same as std::env::temp_dir / $TMPDIR).
    paths.push(std::env::temp_dir().join("verge-mihomo.sock"));
    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        let path = PathBuf::from(tmpdir).join("verge-mihomo.sock");
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    // Legacy location used by older builds.
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

// --- Clash Verge app control ---

fn match_clash_verge_app(executable: &str) -> bool {
    let name = Path::new(executable.trim())
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "clash-verge" | "clash-verge-rev" | "clash verge" | "clash verge rev"
    )
}

#[cfg(target_os = "linux")]
fn owned_process(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    path.metadata()
        .is_ok_and(|m| m.uid() == unsafe { libc::geteuid() })
}

/// Only the current user's desktop executable, never a service or a command argument.
pub fn clash_verge_app_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    #[cfg(target_os = "linux")]
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if !owned_process(&entry.path()) {
                continue;
            }
            // comm is truncated to 15 bytes on Linux; exe preserves the full name.
            let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else {
                continue;
            };
            if match_clash_verge_app(&exe.to_string_lossy()) {
                pids.push(pid);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("/bin/ps")
            .args(["-ww", "-axo", "uid=,pid=,comm="])
            .output()
        {
            let uid = unsafe { libc::geteuid() };
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(pid) = app_pid_from_ps(line, uid) {
                    pids.push(pid);
                }
            }
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(any(target_os = "macos", test))]
fn app_pid_from_ps(line: &str, uid: u32) -> Option<u32> {
    let owner: u32 = line.split_whitespace().next()?.parse().ok()?;
    let tail = line
        .trim()
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start();
    let (pid, executable) = tail.split_once(char::is_whitespace)?;
    (owner == uid && match_clash_verge_app(executable))
        .then(|| pid.parse().ok())
        .flatten()
}

pub fn stop_clash_verge() -> Result<usize> {
    let pids = clash_verge_app_pids();
    ensure!(
        !pids.is_empty(),
        "未找到当前用户正在运行的 Clash Verge 应用"
    );
    #[cfg(target_os = "macos")]
    {
        // Use the application's Quit handler so it can restore system proxy settings.
        let script = format!(
            "with timeout of 3 seconds\ntell application id \"{APP_ID}\" to quit\nend timeout"
        );
        let output = Command::new("/usr/bin/osascript")
            .args(["-e", &script])
            .output()
            .context("无法请求 Clash Verge 退出")?;
        ensure!(
            output.status.success(),
            "Clash Verge 退出请求失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    #[cfg(target_os = "linux")]
    for pid in &pids {
        if unsafe { libc::kill(*pid as libc::pid_t, libc::SIGTERM) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).with_context(|| format!("无法停止 Clash Verge (PID {pid})"));
            }
        }
    }
    // Give the app time to flush settings and shut down its core. Never kill newly
    // started instances or forcibly terminate an app still saving configuration.
    for _ in 0..50 {
        if !clash_verge_app_pids().iter().any(|pid| pids.contains(pid)) {
            return Ok(pids.len());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::bail!("Clash Verge 在 5 秒内未退出；请检查应用状态后重试")
}

#[cfg(target_os = "macos")]
fn start_clash_verge() -> Result<()> {
    let status = Command::new("/usr/bin/open")
        .args(["-b", APP_ID])
        .status()
        .context("无法通过 Launch Services 启动 Clash Verge")?;
    ensure!(status.success(), "Launch Services 启动 Clash Verge 失败");
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_launcher(pid: u32) -> Result<Command> {
    linux_launcher_at(&PathBuf::from(format!("/proc/{pid}")))
}

#[cfg(target_os = "linux")]
fn linux_launcher_at(proc: &Path) -> Result<Command> {
    use std::os::unix::ffi::OsStrExt;
    let environ = std::fs::read(proc.join("environ"))
        .context("无法读取 Clash Verge 的桌面会话环境，未执行重启")?;
    let vars: Vec<_> = environ
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let index = entry.iter().position(|b| *b == b'=')?;
            Some((&entry[..index], &entry[index + 1..]))
        })
        .collect();
    let appimage = vars
        .iter()
        .find(|(key, value)| *key == b"APPIMAGE" && !value.is_empty());
    let executable = if let Some((_, value)) = appimage {
        PathBuf::from(std::ffi::OsStr::from_bytes(value))
    } else {
        std::fs::read_link(proc.join("exe")).context("无法定位 Clash Verge 可执行文件")?
    };
    ensure!(
        executable.is_file(),
        "启动文件不存在：{}；未执行重启",
        executable.display()
    );
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        executable.metadata()?.permissions().mode() & 0o111 != 0,
        "启动文件不可执行：{}；未执行重启",
        executable.display()
    );
    let mut command = Command::new(executable);
    if let Ok(cwd) = std::fs::read_link(proc.join("cwd")) {
        // AppImage mount directories disappear when the old process exits.
        if appimage.is_some()
            && cwd
                .components()
                .any(|part| part.as_os_str().to_string_lossy().starts_with(".mount_"))
        {
            command.current_dir(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
        } else {
            command.current_dir(cwd);
        }
    }
    // Recreate the GUI session environment when the CLI is run over SSH. Tauri/WebKit
    // also needs session and toolkit variables beyond DISPLAY and DBUS_SESSION_BUS_ADDRESS.
    command.env_clear();
    for (key, value) in &vars {
        command.env(
            std::ffi::OsStr::from_bytes(key),
            std::ffi::OsStr::from_bytes(value),
        );
    }
    let args = std::fs::read(proc.join("cmdline"))?;
    for arg in args
        .split(|b| *b == 0)
        .skip(1)
        .filter(|arg| !arg.is_empty())
    {
        command.arg(std::ffi::OsStr::from_bytes(arg));
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

pub fn restart_clash_verge() -> Result<usize> {
    #[cfg(target_os = "linux")]
    let mut launcher = {
        let pids = clash_verge_app_pids();
        ensure!(
            pids.len() == 1,
            "重启需要当前用户恰好运行一个 Clash Verge 应用"
        );
        // Capture the AppImage and session before stopping/unmounting it.
        linux_launcher(pids[0])?
    };
    let stopped = stop_clash_verge()?;
    // Let the app's service, singleton lock, and proxy state settle before relaunch.
    std::thread::sleep(std::time::Duration::from_millis(500));
    #[cfg(target_os = "macos")]
    {
        start_clash_verge()?;
    }
    #[cfg(target_os = "linux")]
    let mut child = launcher.spawn().context("无法重新启动 Clash Verge")?;
    // Cold starts can take longer on Linux (WebKit/service setup) and macOS
    // (Launch Services); do not report a successful `open` as a running app.
    for _ in 0..300 {
        #[cfg(target_os = "linux")]
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "Clash Verge 启动后退出：{status}");
        }
        if !clash_verge_app_pids().is_empty() {
            return Ok(stopped);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::bail!("未检测到重启后的 Clash Verge 应用，请检查桌面会话与启动环境")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn app_matching_uses_executable_not_arguments_or_substrings() {
        for name in [
            "clash-verge",
            "clash-verge-rev",
            "/Applications/Clash Verge.app/Contents/MacOS/clash-verge",
            "/opt/custom service/clash-verge",
        ] {
            assert!(match_clash_verge_app(name), "{name}");
        }
        for name in [
            "clash-verge-service",
            "verge-mihomo",
            "clash-verge-helper",
            "sh -c clash-verge",
            "/tmp/clash-verge/other",
        ] {
            assert!(!match_clash_verge_app(name), "{name}");
        }
        assert_eq!(
            app_pid_from_ps(
                " 501   123 /Applications/Clash Verge.app/Contents/MacOS/clash-verge",
                501
            ),
            Some(123)
        );
        assert_eq!(
            app_pid_from_ps(
                " 502   123 /Applications/Clash Verge.app/Contents/MacOS/clash-verge",
                501
            ),
            None
        );
        assert_eq!(app_pid_from_ps(" 501   123 /bin/sh", 501), None);
        assert_eq!(app_pid_from_ps("invalid", 501), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn restart_captures_appimage_and_original_desktop_session() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("Clash Verge.AppImage");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            dir.path().join("environ"),
            format!(
                "APPIMAGE={}\0DISPLAY=:7\0XDG_RUNTIME_DIR=/run/user/1000\0",
                executable.display()
            ),
        )
        .unwrap();
        std::os::unix::fs::symlink("/tmp/.mount_verge/usr/bin", dir.path().join("cwd")).unwrap();
        std::fs::write(
            dir.path().join("cmdline"),
            b"/tmp/.mount_abc/clash-verge\0--flag\0with spaces\0",
        )
        .unwrap();
        let command = linux_launcher_at(dir.path()).unwrap();
        assert_eq!(command.get_program(), executable.as_os_str());
        assert!(
            !command
                .get_current_dir()
                .unwrap()
                .starts_with("/tmp/.mount_verge")
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--flag", "with spaces"]
        );
        let env: Vec<_> = command.get_envs().collect();
        assert!(env.contains(&(
            std::ffi::OsStr::new("DISPLAY"),
            Some(std::ffi::OsStr::new(":7"))
        )));
        assert!(env.contains(&(std::ffi::OsStr::new("WAYLAND_DISPLAY"), None)));
        std::fs::remove_file(executable).unwrap();
        assert!(linux_launcher_at(dir.path()).is_err());
    }

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
