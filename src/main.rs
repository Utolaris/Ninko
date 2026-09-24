use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use comfy_table::{Table, presets::UTF8_FULL_CONDENSED};
use console::style;
mod platform;
mod terminal;
use futures_util::{StreamExt, stream};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap, io::IsTerminal, path::PathBuf, process::ExitCode, time::Duration,
};

/// Human UI: run `ninko` with no args (Chinese TUI).
/// Agent CLI: subcommands print a single JSON object on stdout (English help).
#[derive(Parser)]
#[command(
    name = "ninko",
    bin_name = "ninko",
    version,
    about = "Clash Verge controller. Never switch nodes or stop/restart the app unless the user explicitly authorizes it.",
    long_about = "Clash Verge controller.\n\n\
Permission boundary: Never switch nodes or stop/restart Clash Verge unless the user explicitly authorizes that action.\n\
\n\
Audience:\n\
  - `ninko` (no args, TTY): interactive Chinese menu for humans.\n\
  - `ninko <command>`: machine-readable JSON on stdout for LLM/agents.\n\
\n\
Agent workflow (LLM):\n\
  1. ALWAYS run `ninko test` first — every node gets a stable `index` (1-based).\n\
  2. Pick a node from `passed` (or inspect `failed`).\n\
  3. Switch with that index: `ninko switch <index>` or `ninko switch <group> <index>`.\n\
     Full node names still work: `ninko switch <group> '<node name>'`.\n\
\n\
JSON envelope: success objects include `\"ok\": true` (except `list`/`test` payloads);\n\
errors are `{\"ok\": false, \"error\": \"...\"}` with exit code 1.\n\
Requires Clash Verge Rev installed and running (core reachable via Unix socket or HTTP)."
)]
struct Cli {
    /// Clash Verge data directory (overrides auto-discovery)
    #[arg(long, env = "CLASH_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,
    /// Explicit Unix socket path to mihomo external controller
    #[arg(long, env = "CLASH_SOCK", global = true)]
    sock: Option<PathBuf>,
    /// Explicit HTTP controller URL (takes priority over auto socket discovery)
    #[arg(long, env = "CLASH_API", global = true)]
    api: Option<String>,
    /// Controller secret (Bearer token)
    #[arg(long, env = "CLASH_SECRET", global = true, hide_env_values = true)]
    secret: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print current profile, providers, groups, and nodes as JSON
    List,
    /// Measure node latency. Stdout: {"passed":[...],"failed":[...]} — each node has `index`
    Test {
        /// Limit to one provider
        #[arg(long, conflicts_with = "group")]
        provider: Option<String>,
        /// Limit to one policy group (expands nested groups)
        #[arg(long)]
        group: Option<String>,
        /// Probe URL
        #[arg(long, default_value = "https://www.gstatic.com/generate_204")]
        url: String,
        /// Per-node timeout in milliseconds
        #[arg(long, default_value_t = 5000, value_parser = clap::value_parser!(u64).range(1..=120000))]
        timeout: u64,
        /// Concurrent probes
        #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u16).range(1..=64))]
        concurrency: u16,
    },
    /// Switch selection. Prefer `ninko switch <index>` after `ninko test` (index from that run)
    Switch {
        /// Policy group name, or a bare test `index` (digits) to resolve group+node
        group: Option<String>,
        /// Node full name, or a test `index` (digits) from the last `ninko test`
        node: Option<String>,
    },
    /// Restart or stop the Clash Verge desktop app
    Control { action: ControlAction },
    /// Print absolute path of the global Merge YAML as JSON {"path":"..."}
    MergePath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ControlAction {
    /// Restart the Clash Verge app
    Restart,
    /// Stop / quit the Clash Verge app
    Stop,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Audience {
    /// Interactive Chinese TUI
    Human,
    /// JSON on stdout for agents
    Agent,
}

#[derive(Debug, Deserialize)]
struct Profiles {
    current: Option<String>,
    #[serde(default)]
    items: Vec<Profile>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Profile {
    uid: String,
    name: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    file: Option<String>,
}

fn data_dir(cli: &Cli) -> Result<PathBuf> {
    platform::data_dir(cli.data_dir.as_deref())
}

fn profiles(dir: &std::path::Path) -> Result<Profiles> {
    let path = dir.join("profiles.yaml");
    serde_yaml::from_str(&std::fs::read_to_string(&path).with_context(|| {
        format!(
            "找不到 Clash Verge 配置 {}，请确认已安装并运行过 Clash Verge Rev，或用 --data-dir 指定数据目录",
            path.display()
        )
    })?)
    .context("无法解析 profiles.yaml")
}

fn merge_path(dir: &std::path::Path, profiles: &Profiles) -> Result<PathBuf> {
    let entry = profiles
        .items
        .iter()
        .find(|p| p.uid == "Merge" && p.kind == "merge")
        .context("profiles.yaml 中没有全局 Merge 配置")?;
    let file = entry.file.as_ref().context("全局 Merge 未指定文件")?;
    dir.join("profiles")
        .join(file)
        .canonicalize()
        .context("全局 Merge 文件不存在")
}

struct Api {
    client: Client,
    base: Url,
    secret: Option<String>,
}
impl Api {
    fn new(cli: &Cli) -> Result<Self> {
        let mut builder = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(130));
        let dir = data_dir(cli)?;
        let mut config = if cli.api.is_none() {
            platform::controller(&dir)?
        } else {
            platform::Controller::default()
        };
        let running_core = if cli.api.is_none() && cli.sock.is_none() {
            platform::running_cores()
                .into_iter()
                .find(|core| platform::usable_socket(&core.socket))
        } else {
            None
        };
        if let Some(core) = &running_core {
            if let Some(path) = &core.config
                && let Ok(runtime_config) = platform::controller_file(path)
            {
                config.address = runtime_config.address.or(config.address);
                config.secret = runtime_config.secret.or(config.secret);
            }
            // The live core arguments are authoritative. Clash Verge Rev 2.5.5
            // service mode keeps its socket under /var/run, outside app data.
            config.socket = Some(core.socket.clone());
        }
        let sock = cli.sock.clone().or_else(|| {
            if cli.api.is_some() {
                return None;
            }
            platform::socket_candidates(&dir, config.socket.as_deref())
                .into_iter()
                .find(|path| platform::usable_socket(path))
        });
        let base = if let Some(sock) = sock {
            #[cfg(unix)]
            {
                builder = builder.unix_socket(sock);
            }
            #[cfg(not(unix))]
            {
                let _ = sock;
                bail!("Unix sockets are not supported on this platform; use --api");
            }
            Url::parse("http://localhost")?
        } else {
            Url::parse(&platform::controller_url(
                cli.api.as_deref().or(config.address.as_deref()),
            ))?
        };
        ensure!(
            matches!(base.scheme(), "http" | "https"),
            "controller URL must be http/https"
        );
        Ok(Self {
            client: builder.build()?,
            base,
            secret: cli.secret.clone().or(config.secret),
        })
    }
    async fn reachable(&self) -> bool {
        let mut url = self.base.clone();
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.clear().extend(["version"]);
        } else {
            return false;
        }
        let request = self.client.get(url);
        let request = if let Some(secret) = &self.secret {
            request.bearer_auth(secret)
        } else {
            request
        };
        // Any HTTP response proves the core is listening, including 401/404.
        request.send().await.is_ok()
    }
    async fn call(
        &self,
        method: Method,
        parts: &[&str],
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid controller URL"))?
            .clear()
            .extend(parts);
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }
        let mut req = self.client.request(method, url);
        if let Some(secret) = &self.secret {
            req = req.bearer_auth(secret);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req
            .send()
            .await
            .map_err(|e| e.without_url())
            .context(
                "cannot reach Clash Verge controller; is the app running, and are socket/API/secret correct?",
            )?;
        let status = response.status();
        if matches!(status.as_u16(), 503 | 504) {
            bail!(
                "upstream unreachable or probe timeout (HTTP {})",
                status.as_u16()
            );
        }
        if !status.is_success() {
            bail!(
                "Clash Verge API returned {status}{}",
                if status.as_u16() == 401 {
                    " — set CLASH_SECRET"
                } else {
                    ""
                }
            );
        }
        if status.as_u16() == 204 {
            return Ok(Value::Null);
        }
        let bytes = response
            .bytes()
            .await
            .context("Clash Verge returned an empty body")?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).context("Clash Verge returned invalid JSON")
    }
    async fn get(&self, parts: &[&str]) -> Result<Value> {
        self.call(Method::GET, parts, &[], None).await
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Node {
    name: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    now: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    all: Option<Vec<String>>,
}
#[derive(Deserialize, Serialize)]
struct Provider {
    #[serde(rename = "vehicleType", default)]
    vehicle_type: String,
    #[serde(default)]
    proxies: Vec<Node>,
}
#[derive(Deserialize)]
struct ProviderResponse {
    providers: BTreeMap<String, Provider>,
}
#[derive(Deserialize)]
struct ProxyResponse {
    proxies: BTreeMap<String, Node>,
}

struct Runtime {
    providers: BTreeMap<String, Provider>,
    proxies: BTreeMap<String, Node>,
}
impl Runtime {
    async fn read(api: &Api) -> Result<Self> {
        let providers = api.get(&["providers", "proxies"]).await?;
        let proxies = api.get(&["proxies"]).await?;
        let mut providers: ProviderResponse = serde_json::from_value(providers)?;
        providers
            .providers
            .retain(|_, p| p.vehicle_type != "Compatible");
        Ok(Self {
            providers: providers.providers,
            proxies: serde_json::from_value::<ProxyResponse>(proxies)?.proxies,
        })
    }
    fn targets(&self, provider: Option<&str>, group: Option<&str>) -> Result<Vec<Target>> {
        let mut result = Vec::new();
        if let Some(group) = group {
            let mut visited = std::collections::BTreeSet::new();
            self.expand(group, &mut visited, &mut result)?;
            return Ok(result);
        }
        if let Some(name) = provider {
            ensure!(
                self.providers.contains_key(name),
                "provider not loaded: {name}"
            );
        }
        for (name, p) in &self.providers {
            if provider.is_none_or(|selected| selected == name) {
                for node in &p.proxies {
                    if testable(node) {
                        result.push(Target {
                            provider: Some(name.clone()),
                            name: node.name.clone(),
                        });
                    }
                }
            }
        }
        if provider.is_none() {
            for node in self.proxies.values() {
                if testable(node) && !result.iter().any(|t| t.name == node.name) {
                    result.push(Target {
                        provider: None,
                        name: node.name.clone(),
                    });
                }
            }
        }
        Ok(result)
    }
    fn expand(
        &self,
        name: &str,
        visited: &mut std::collections::BTreeSet<String>,
        result: &mut Vec<Target>,
    ) -> Result<()> {
        if !visited.insert(name.to_owned()) {
            return Ok(());
        }
        let node = self
            .proxies
            .get(name)
            .with_context(|| format!("node or group not found: {name}"))?;
        if let Some(members) = &node.all {
            for member in members {
                self.expand(member, visited, result)?;
            }
        } else if testable(node) {
            result.push(Target {
                provider: None,
                name: name.to_owned(),
            });
        }
        Ok(())
    }
}
fn testable(node: &Node) -> bool {
    node.all.is_none()
        && !matches!(
            node.kind.to_ascii_uppercase().as_str(),
            "DIRECT" | "REJECT" | "REJECTDROP" | "PASS" | "COMPATIBLE"
        )
}

/// mihomo synthesizes GLOBAL; only user-defined Selector groups are real.
fn is_real_group(name: &str, node: &Node) -> bool {
    node.kind == "Selector" && !name.eq_ignore_ascii_case("GLOBAL")
}

#[derive(Clone, Serialize)]
struct Target {
    provider: Option<String>,
    name: String,
}
#[derive(Serialize)]
struct Measurement {
    #[serde(flatten)]
    target: Target,
    delay_ms: Option<u64>,
    error: Option<String>,
}

async fn measure(api: &Api, target: Target, url: &str, timeout: u64) -> Measurement {
    let parts = match &target.provider {
        Some(p) => vec!["providers", "proxies", p, &target.name, "healthcheck"],
        None => vec!["proxies", &target.name, "delay"],
    };
    let result = api
        .call(
            Method::GET,
            &parts,
            &[("url", url.to_owned()), ("timeout", timeout.to_string())],
            None,
        )
        .await
        .and_then(|v| {
            v["delay"]
                .as_u64()
                .filter(|d| (1..=1000).contains(d))
                .context("timeout: delay exceeds 1000 ms or no valid delay")
        });
    match result {
        Ok(delay) => Measurement {
            target,
            delay_ms: Some(delay),
            error: None,
        },
        Err(e) => Measurement {
            target,
            delay_ms: None,
            error: Some(e.to_string()),
        },
    }
}

async fn choose(prompt: &str, items: &[String], default: usize) -> Result<Option<String>> {
    ensure!(!items.is_empty(), "{prompt}: no options");
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "interactive picker needs a TTY; use: ninko switch <group> <node>"
    );
    let selection = terminal::select(prompt, items, default, true, None).await?;
    Ok(selection.map(|i| items[i].clone()))
}

fn result_line(ok: bool, text: &str) -> String {
    if ok {
        format!("  {} {text}", style("✓").green().bold())
    } else {
        format!("  {} {text}", style("✗").red().bold())
    }
}

async fn switch(
    api: &Api,
    runtime: &Runtime,
    group: Option<String>,
    node: Option<String>,
    audience: Audience,
) -> Result<Value> {
    // Agent shortcut: `ninko switch <index>` — index from the last `ninko test`.
    let (group, node) = match (group, node) {
        (Some(g), None) if g.chars().all(|c| c.is_ascii_digit()) && !g.is_empty() => {
            let index: u32 = g.parse().context("test index out of range")?;
            let (resolved_group, name) = resolve_test_index(index, None)?;
            ensure!(
                resolved_group.is_some(),
                "test index {index} has no group; run `ninko test --group <name>` or pass the group: ninko switch <group> {index}"
            );
            (resolved_group, Some(name))
        }
        (Some(g), Some(n)) if n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => {
            let index: u32 = n.parse().context("test index out of range")?;
            let (_, name) = resolve_test_index(index, Some(&g))?;
            (Some(g), Some(name))
        }
        (group, node) => (group, node),
    };
    let groups: Vec<_> = runtime
        .proxies
        .iter()
        .filter(|(name, n)| is_real_group(name, n))
        .map(|(name, _)| name.clone())
        .collect();
    let group = match group {
        Some(g) => g,
        None => match choose("选择策略组", &groups, 0).await? {
            Some(group) => group,
            None => return Ok(json!({"ok": false, "cancelled": true})),
        },
    };
    let info = runtime.proxies.get(&group).context("group not found")?;
    ensure!(
        info.kind == "Selector",
        "only Selector groups are supported"
    );
    let members = info.all.as_ref().context("group has no members")?;
    let node = match node {
        Some(n) => n,
        None => {
            let labels: Vec<_> = members
                .iter()
                .map(|name| {
                    if info.now.as_ref() == Some(name) {
                        format!("{name}  ← 当前使用")
                    } else {
                        name.clone()
                    }
                })
                .collect();
            ensure!(
                std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
                "interactive picker needs a TTY; use: ninko switch <group> <node>"
            );
            let delays = stream::iter(members.iter().enumerate())
                .map(|(index, name)| async move {
                    let target = Target {
                        provider: None,
                        name: name.clone(),
                    };
                    let result = tokio::time::timeout(
                        Duration::from_secs(5),
                        measure(api, target, "https://www.gstatic.com/generate_204", 5000),
                    )
                    .await;
                    (index, result.ok().and_then(|result| result.delay_ms))
                })
                .buffer_unordered(8);
            let selected = terminal::select_live(
                &group,
                &labels,
                members.iter().position(|n| Some(n) == info.now.as_ref()),
                delays,
            )
            .await?;
            match selected {
                Some(index) => members[index].clone(),
                None => return Ok(json!({"ok": false, "cancelled": true})),
            }
        }
    };
    ensure!(
        members.contains(&node),
        "node {node} is not in group {group}"
    );
    api.call(
        Method::PUT,
        &["proxies", &group],
        &[],
        Some(json!({"name": node})),
    )
    .await?;
    let updated = api.get(&["proxies", &group]).await?;
    ensure!(
        updated["now"].as_str() == Some(&node),
        "switch sent but readback mismatch; check Clash Verge"
    );
    let payload = json!({"ok": true, "group": group, "node": node});
    if audience == Audience::Human {
        terminal::show_result(&[result_line(
            true,
            &format!("{group} → {}", style(&node).bold()),
        )])?;
    }
    Ok(payload)
}

async fn wait_for_controller(cli: &Cli) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(api) = Api::new(cli) {
            let reachable = tokio::time::timeout(Duration::from_millis(500), api.reachable())
                .await
                .unwrap_or(false);
            if reachable {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "Clash Verge restarted, but its controller did not become reachable within 30 seconds"
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn control_command(cli: &Cli, action: ControlAction, audience: Audience) -> Result<Value> {
    let payload = match action {
        ControlAction::Stop => {
            let stopped = platform::stop_clash_verge().context("failed to stop Clash Verge")?;
            json!({"ok": true, "action": "stop", "signaled": stopped})
        }
        ControlAction::Restart => {
            let stopped =
                platform::restart_clash_verge().context("failed to restart Clash Verge")?;
            wait_for_controller(cli).await?;
            json!({"ok": true, "action": "restart", "signaled": stopped})
        }
    };
    if audience == Audience::Human {
        let text = match action {
            ControlAction::Stop => "Clash Verge 已退出",
            ControlAction::Restart => "Clash Verge 已重启",
        };
        terminal::show_result(&[result_line(true, text)])?;
    }
    Ok(payload)
}

fn list_json(runtime: &Runtime, current: Option<&Profile>) -> Value {
    let groups: BTreeMap<_, _> = runtime
        .proxies
        .iter()
        .filter(|(name, n)| n.all.is_some() && is_real_group(name, n))
        .collect();
    let inline: Vec<_> = runtime
        .proxies
        .values()
        .filter(|n| {
            testable(n)
                && !runtime
                    .providers
                    .values()
                    .any(|p| p.proxies.iter().any(|pn| pn.name == n.name))
        })
        .collect();
    json!({
        "ok": true,
        "current_profile": current,
        "groups": groups,
        "providers": runtime.providers,
        "inline_nodes": inline,
    })
}

fn print_list_human(runtime: &Runtime, current: Option<&Profile>) {
    if let Some(p) = current {
        println!(
            "当前 Profile：{} [{}]",
            p.name.as_deref().unwrap_or(&p.uid),
            p.kind
        );
    }
    println!("\n  {}", style("当前策略组").cyan().bold());
    let mut selections = table(&["策略组", "当前节点"]);
    for (name, n) in runtime
        .proxies
        .iter()
        .filter(|(name, n)| n.all.is_some() && is_real_group(name, n))
    {
        selections.add_row([name.as_str(), n.now.as_deref().unwrap_or("无固定选择")]);
    }
    println!("{selections}");
    for (name, p) in &runtime.providers {
        println!(
            "\n订阅：{name} [{}] — {} 个节点",
            p.vehicle_type,
            p.proxies.len()
        );
        print_nodes(p.proxies.iter(), runtime);
    }
    let inline: Vec<_> = runtime
        .proxies
        .values()
        .filter(|n| {
            testable(n)
                && !runtime
                    .providers
                    .values()
                    .any(|p| p.proxies.iter().any(|pn| pn.name == n.name))
        })
        .collect();
    if !inline.is_empty() {
        println!("\n运行配置内置节点：");
        print_nodes(inline.into_iter(), runtime);
    }
}

fn test_groups(results: &[Measurement], group: Option<&str>) -> Value {
    let mut passed = Vec::new();
    let mut failed = Vec::new();
    // 1-based index across the whole run (passed first after sort, then failed).
    let mut index = 0u32;
    let mut cache = Vec::new();
    for r in results {
        index += 1;
        let provider = r.target.provider.clone();
        let name = r.target.name.clone();
        if r.delay_ms.is_some() {
            passed.push(json!({
                "index": index,
                "provider": provider,
                "name": name,
                "delay_ms": r.delay_ms,
                "group": group,
            }));
        } else {
            failed.push(json!({
                "index": index,
                "provider": provider,
                "name": name,
                "error": r.error.clone().unwrap_or_else(|| "unknown".into()),
                "group": group,
            }));
        }
        cache.push(json!({
            "index": index,
            "provider": r.target.provider,
            "name": r.target.name,
            "delay_ms": r.delay_ms,
            "error": r.error,
            "group": group,
        }));
    }
    let _ = save_last_test(&cache);
    json!({"ok": true, "passed": passed, "failed": failed})
}

fn last_test_path() -> PathBuf {
    if let Ok(dir) = std::env::var("NINKO_STATE_DIR") {
        return PathBuf::from(dir).join("last-test.json");
    }
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("ninko")
        .join("last-test.json")
}

fn save_last_test(entries: &[Value]) -> Result<()> {
    let path = last_test_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({"entries": entries}))?,
    )?;
    Ok(())
}

fn load_last_test() -> Result<Vec<Value>> {
    let path = last_test_path();
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no last test cache at {}; run `ninko test` first",
            path.display()
        )
    })?;
    let doc: Value =
        serde_json::from_str(&text).context("last-test.json is corrupt; re-run `ninko test`")?;
    Ok(doc["entries"].as_array().cloned().unwrap_or_default())
}

/// Resolve a test `index` (digits) to (group, node name). `hint_group` wins when set.
fn resolve_test_index(index: u32, hint_group: Option<&str>) -> Result<(Option<String>, String)> {
    let entries = load_last_test()?;
    let hit = entries
        .iter()
        .find(|e| e["index"].as_u64() == Some(u64::from(index)))
        .with_context(|| format!("test index {index} not in last test; run `ninko test` again"))?;
    let name = hit["name"]
        .as_str()
        .context("test entry has no name")?
        .to_owned();
    let group = hint_group
        .map(str::to_owned)
        .or_else(|| hit["group"].as_str().map(str::to_owned));
    Ok((group, name))
}

async fn execute(cli: &Cli, command: Command, audience: Audience) -> Result<Value> {
    if matches!(command, Command::MergePath) {
        let dir = data_dir(cli)?;
        let path = merge_path(&dir, &profiles(&dir)?)?;
        return Ok(json!({"ok": true, "path": path.display().to_string()}));
    }
    if let Command::Control { action } = command {
        return control_command(cli, action, audience).await;
    }
    let api = Api::new(cli)?;
    let runtime = Runtime::read(&api).await?;
    match command {
        Command::List => {
            let metadata = profiles(&data_dir(cli)?)?;
            let current = metadata
                .items
                .iter()
                .find(|p| Some(&p.uid) == metadata.current.as_ref());
            if audience == Audience::Agent {
                Ok(list_json(&runtime, current))
            } else {
                print_list_human(&runtime, current);
                Ok(Value::Null)
            }
        }
        Command::Test {
            provider,
            group,
            url,
            timeout,
            concurrency,
        } => {
            let test_url = Url::parse(&url).context("invalid probe URL")?;
            ensure!(
                matches!(test_url.scheme(), "http" | "https"),
                "probe URL must be http/https"
            );
            let targets = runtime.targets(provider.as_deref(), group.as_deref())?;
            ensure!(!targets.is_empty(), "no testable nodes");
            if audience == Audience::Human {
                eprintln!(
                    "正在测试 {} 个节点，并发 {}，每个节点超时 {} ms…",
                    targets.len(),
                    concurrency,
                    timeout
                );
            }
            let progress = if audience == Audience::Human {
                ProgressBar::new(targets.len() as u64)
            } else {
                ProgressBar::hidden()
            };
            progress.set_style(
                ProgressStyle::with_template(
                    "  {spinner:.cyan} [{bar:30.cyan/blue}] {pos}/{len} · {elapsed} · {msg}",
                )?
                .progress_chars("━━─"),
            );
            progress.enable_steady_tick(Duration::from_millis(100));
            let mut results: Vec<_> = stream::iter(targets)
                .map(|t| measure(&api, t, &url, timeout))
                .buffer_unordered(concurrency as usize)
                .inspect(|r| {
                    progress.inc(1);
                    progress.set_message(r.target.name.clone());
                })
                .collect()
                .await;
            progress.finish_and_clear();
            results.sort_by_key(|r| r.delay_ms.unwrap_or(u64::MAX));
            if audience == Audience::Agent {
                Ok(test_groups(&results, group.as_deref()))
            } else {
                let passed = results.iter().filter(|r| r.delay_ms.is_some()).count();
                println!(
                    "\n  {}  {} 可用 / {} 失败 · 延迟由低到高",
                    style("测速完成").green().bold(),
                    passed,
                    results.len() - passed
                );
                let mut rows = table(&["序号", "延迟", "订阅", "节点", "状态"]);
                for (i, r) in results.iter().enumerate() {
                    rows.add_row([
                        (i + 1).to_string(),
                        r.delay_ms.map(|d| format!("{d} ms")).unwrap_or("—".into()),
                        r.target.provider.clone().unwrap_or("运行配置".into()),
                        r.target.name.clone(),
                        r.error.clone().unwrap_or("可用".into()),
                    ]);
                }
                println!("{rows}");
                let _ = test_groups(&results, group.as_deref());
                Ok(Value::Null)
            }
        }
        Command::Switch { group, node } => switch(&api, &runtime, group, node, audience).await,
        Command::Control { .. } | Command::MergePath => unreachable!(),
    }
}

fn emit_agent(result: Result<Value>) -> ExitCode {
    match result {
        Ok(value) => {
            if value != Value::Null {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let payload = json!({"ok": false, "error": format!("{e:#}")});
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
            );
            ExitCode::FAILURE
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    // Restore normal Unix CLI behaviour when the pipe closes early (e.g. `head`).
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let mut cli = Cli::parse();
    if let Some(command) = cli.command.take() {
        return emit_agent(execute(&cli, command, Audience::Agent).await);
    }
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return emit_agent(Err(anyhow::anyhow!(
            "no command given; run `ninko --help` for agent commands"
        )));
    }
    if let Err(e) = human_menu(&cli).await {
        eprintln!("\n  {} {e:#}\n", style("提示：").yellow());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn human_menu(cli: &Cli) -> Result<()> {
    println!(
        "\n  {}  {}",
        style("NINKO").cyan().bold(),
        style("节点控制台").bold()
    );
    println!("  {}\n", style("↑↓ 选择 · Enter 确定 · Esc 退出").dim());
    loop {
        let merge_footer = (|| -> Result<String> {
            let dir = data_dir(cli)?;
            Ok(merge_path(&dir, &profiles(&dir)?)?.display().to_string())
        })()
        .unwrap_or_else(|e| format!("无法定位：{e}"));
        let items = [
            "切换节点      搜索并切换当前策略组".to_owned(),
            "查看订阅      完整节点列表与当前选择".to_owned(),
            "Clash Verge    重启 / 退出应用".to_owned(),
            "退出".to_owned(),
        ];
        let selected =
            terminal::select("你想做什么？", &items, 0, false, Some(&merge_footer)).await?;
        match selected {
            Some(0) => {
                if let Err(e) = execute(
                    cli,
                    Command::Switch {
                        group: None,
                        node: None,
                    },
                    Audience::Human,
                )
                .await
                {
                    terminal::show_result(&[result_line(false, &format!("{e:#}"))])?;
                }
            }
            Some(1) => {
                if let Err(e) = execute(cli, Command::List, Audience::Human).await {
                    eprintln!("\n  {} {e:#}\n", style("提示：").yellow());
                }
                terminal::wait_key()?;
            }
            Some(2) => {
                if let Err(e) = verge_control_menu(cli).await {
                    terminal::show_result(&[result_line(false, &format!("{e:#}"))])?;
                }
            }
            _ => return Ok(()),
        }
    }
}

async fn verge_control_menu(cli: &Cli) -> Result<()> {
    let items = [
        "重启 Clash Verge    关闭后再启动应用".to_owned(),
        "关闭 Clash Verge    退出应用（核心服务仍可运行）".to_owned(),
        "返回".to_owned(),
    ];
    let selected = terminal::select("Clash Verge", &items, 0, false, None).await?;
    match selected {
        Some(0) => execute(
            cli,
            Command::Control {
                action: ControlAction::Restart,
            },
            Audience::Human,
        )
        .await
        .map(|_| ()),
        Some(1) => execute(
            cli,
            Command::Control {
                action: ControlAction::Stop,
            },
            Audience::Human,
        )
        .await
        .map(|_| ()),
        _ => Ok(()),
    }
}

fn table(headers: &[&str]) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED).set_header(headers);
    table
}

fn print_nodes<'a>(nodes: impl Iterator<Item = &'a Node>, runtime: &Runtime) {
    let mut rows = table(&["序号", "节点", "协议", "当前使用"]);
    for (i, n) in nodes.enumerate() {
        let selected: Vec<_> = runtime
            .proxies
            .iter()
            .filter(|(_, g)| g.now.as_ref() == Some(&n.name))
            .map(|(name, _)| name.as_str())
            .collect();
        rows.add_row([
            (i + 1).to_string(),
            n.name.clone(),
            n.kind.clone(),
            selected.join(" / "),
        ]);
    }
    println!("{rows}");
}
