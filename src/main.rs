use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use comfy_table::{Table, presets::UTF8_FULL_CONDENSED};
use console::style;
mod platform;
mod terminal;
use futures_util::{StreamExt, stream};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::IsTerminal, path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(
    name = "Ninko",
    bin_name = "ninko",
    version,
    about = "Clash Verge 节点管理（需要已运行的 Clash Verge）"
)]
struct Cli {
    #[arg(long, env = "CLASH_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, env = "CLASH_SOCK", global = true)]
    sock: Option<PathBuf>,
    #[arg(
        long,
        env = "CLASH_API",
        global = true,
        help = "显式指定 HTTP 控制器，优先于默认 socket"
    )]
    api: Option<String>,
    #[arg(long, env = "CLASH_SECRET", global = true, hide_env_values = true)]
    secret: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 列出当前 Profile、已加载订阅及完整节点列表
    List {
        #[arg(long)]
        json: bool,
    },
    /// 并发测试运行节点的延迟（毫秒），不是下载带宽
    Test {
        #[arg(long, conflicts_with = "group")]
        provider: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long, default_value = "https://www.gstatic.com/generate_204")]
        url: String,
        #[arg(long, default_value_t = 5000, value_parser = clap::value_parser!(u64).range(1..=120000))]
        timeout: u64,
        #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u16).range(1..=64))]
        concurrency: u16,
        #[arg(long)]
        json: bool,
    },
    /// 快速切换：省略参数时使用可搜索菜单
    Switch {
        group: Option<String>,
        node: Option<String>,
    },
    /// 输出全局扩展 Merge YAML 的绝对路径
    MergePath,
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
            "找不到 Clash Verge 配置 {}，请检查安装或 --data-dir",
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
        let config = if cli.api.is_none() {
            platform::controller(&dir)?
        } else {
            platform::Controller::default()
        };
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
                bail!("本平台不支持 Unix socket，请使用 --api");
            }
            Url::parse("http://localhost")?
        } else {
            Url::parse(&platform::controller_url(
                cli.api.as_deref().or(config.address.as_deref()),
            ))?
        };
        ensure!(
            matches!(base.scheme(), "http" | "https"),
            "控制器必须使用 HTTP/HTTPS"
        );
        Ok(Self {
            client: builder.build()?,
            base,
            secret: cli.secret.clone().or(config.secret),
        })
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
            .map_err(|_| anyhow::anyhow!("无效控制器地址"))?
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
            .context("无法连接 Clash Verge；请确认应用已运行及控制器地址正确")?;
        let status = response.status();
        if matches!(status.as_u16(), 503 | 504) {
            bail!("节点不可达或测速超时（HTTP {}）", status.as_u16());
        }
        if !status.is_success() {
            bail!(
                "Clash Verge API 返回 {status}{}",
                if status.as_u16() == 401 {
                    "，请设置 CLASH_SECRET"
                } else {
                    ""
                }
            );
        }
        if status.as_u16() == 204 {
            return Ok(Value::Null);
        }
        response.json().await.context("Clash Verge 返回无效 JSON")
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
        // Compatible 是核心合成的集合，不能当作真正订阅重复展示。
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
                "找不到运行中的订阅：{name}"
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
            .with_context(|| format!("找不到节点或策略组：{name}"))?;
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
                .filter(|d| *d > 0)
                .context("超时或无有效延迟")
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
    ensure!(!items.is_empty(), "{prompt}：没有可选项");
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "非交互终端请指定完整命令：ninko switch <策略组> <节点>"
    );
    let selection = terminal::select(prompt, items, default, true, None).await?;
    Ok(selection.map(|i| items[i].clone()))
}

async fn switch(
    api: &Api,
    runtime: &Runtime,
    group: Option<String>,
    node: Option<String>,
    interactive: bool,
) -> Result<()> {
    let groups: Vec<_> = runtime
        .proxies
        .iter()
        .filter(|(_, n)| n.kind == "Selector")
        .map(|(name, _)| name.clone())
        .collect();
    let group = match group {
        Some(g) => g,
        None => match choose("选择策略组（输入可搜索）", &groups, 0).await? {
            Some(group) => group,
            None => return Ok(()),
        },
    };
    let info = runtime.proxies.get(&group).context("找不到策略组")?;
    ensure!(info.kind == "Selector", "只支持手动选择策略组 Selector");
    let members = info.all.as_ref().context("策略组没有成员列表")?;
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
                "非交互终端请指定完整命令：ninko switch <策略组> <节点>"
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
                &format!("{group} · 输入搜索 / ↑↓ 选择 / Enter 确定 / Esc 返回"),
                &labels,
                members.iter().position(|n| Some(n) == info.now.as_ref()),
                delays,
            )
            .await?;
            match selected {
                Some(index) => members[index].clone(),
                None => return Ok(()),
            }
        }
    };
    ensure!(members.contains(&node), "节点 {node} 不属于策略组 {group}");
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
        "切换请求已发送，但读回结果不一致，请检查 Clash Verge"
    );
    let line = format!(
        "  {} {group} → {}",
        style("✓ 已切换").green().bold(),
        style(node).bold()
    );
    if interactive {
        // 提示只出现在节点切换界面，离开时清除，不带回主菜单
        terminal::show_result(&[line])?;
    } else {
        println!("\n{line}\n");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // 管道下游提前关闭（例如 head）时按普通 Unix CLI 行为退出。
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let mut cli = Cli::parse();
    if let Some(command) = cli.command.take() {
        return execute(&cli, command, false).await;
    }
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "请指定命令，运行 ninko --help 查看帮助"
    );
    println!(
        "\n  {}  {}",
        style("NINKO").cyan().bold(),
        style("节点控制台").bold()
    );
    println!("  {}\n", style("↑↓ 选择 · Enter 确定 · Esc 退出").dim());
    loop {
        let merge_footer = (|| -> Result<String> {
            let dir = data_dir(&cli)?;
            Ok(merge_path(&dir, &profiles(&dir)?)?.display().to_string())
        })()
        .unwrap_or_else(|e| format!("无法定位：{e}"));
        let items = [
            "切换节点      搜索并切换当前策略组",
            "查看订阅      完整节点列表与当前选择",
            "退出",
        ]
        .map(str::to_owned);
        let selected =
            terminal::select("你想做什么？", &items, 0, false, Some(&merge_footer)).await?;
        let command = match selected {
            Some(0) => Command::Switch {
                group: None,
                node: None,
            },
            Some(1) => Command::List { json: false },
            _ => return Ok(()),
        };
        let switching = matches!(command, Command::Switch { .. });
        if let Err(e) = execute(&cli, command, true).await {
            if switching {
                // 切换失败提示同样只留在切换界面
                terminal::show_result(&[format!("  {} {e:#}", style("提示：").yellow())])?;
            } else {
                eprintln!("\n  {} {e:#}\n", style("提示：").yellow());
            }
        }
        if !switching {
            terminal::wait_key()?;
        }
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

async fn execute(cli: &Cli, command: Command, interactive: bool) -> Result<()> {
    if matches!(command, Command::MergePath) {
        let dir = data_dir(cli)?;
        println!("{}", merge_path(&dir, &profiles(&dir)?)?.display());
        return Ok(());
    }
    let api = Api::new(cli)?;
    let runtime = Runtime::read(&api).await?;
    match command {
        Command::List { json: as_json } => {
            let metadata = profiles(&data_dir(cli)?)?;
            let current = metadata
                .items
                .iter()
                .find(|p| Some(&p.uid) == metadata.current.as_ref());
            let groups: BTreeMap<_, _> = runtime
                .proxies
                .iter()
                .filter(|(_, n)| n.all.is_some())
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
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"current_profile": current, "providers": runtime.providers, "inline_nodes": inline, "groups": groups})
                    )?
                );
            } else {
                if let Some(p) = current {
                    println!(
                        "当前 Profile：{} [{}]",
                        p.name.as_deref().unwrap_or(&p.uid),
                        p.kind
                    );
                }
                println!("\n  {}", style("当前策略组").cyan().bold());
                let mut selections = table(&["策略组", "当前节点"]);
                for (name, n) in &groups {
                    selections.add_row([name.as_str(), n.now.as_deref().unwrap_or("无固定选择")]);
                }
                println!("{selections}");
                for (name, p) in &runtime.providers {
                    println!(
                        "\n订阅：{name} [{}] — {} 个节点",
                        p.vehicle_type,
                        p.proxies.len()
                    );
                    print_nodes(p.proxies.iter(), &runtime);
                }
                if !inline.is_empty() {
                    println!("\n运行配置内置节点：");
                    print_nodes(inline.into_iter(), &runtime);
                }
            }
        }
        Command::Test {
            provider,
            group,
            url,
            timeout,
            concurrency,
            json: as_json,
        } => {
            let test_url = Url::parse(&url).context("测速 URL 无效")?;
            ensure!(
                matches!(test_url.scheme(), "http" | "https"),
                "测速 URL 必须使用 HTTP/HTTPS"
            );
            let targets = runtime.targets(provider.as_deref(), group.as_deref())?;
            ensure!(!targets.is_empty(), "没有可测速节点");
            eprintln!(
                "正在测试 {} 个节点，并发 {}，每个节点超时 {} ms…",
                targets.len(),
                concurrency,
                timeout
            );
            let progress = if !as_json {
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
            if as_json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                let passed = results.iter().filter(|r| r.delay_ms.is_some()).count();
                println!(
                    "\n  {}  {} 可用 / {} 失败 · 延迟由低到高",
                    style("测速完成").green().bold(),
                    passed,
                    results.len() - passed
                );
                let mut rows = table(&["延迟", "订阅", "节点", "状态"]);
                for r in &results {
                    rows.add_row([
                        r.delay_ms.map(|d| format!("{d} ms")).unwrap_or("—".into()),
                        r.target.provider.clone().unwrap_or("运行配置".into()),
                        r.target.name.clone(),
                        r.error.clone().unwrap_or("可用".into()),
                    ]);
                }
                println!("{rows}");
            }
            ensure!(
                results.iter().any(|r| r.delay_ms.is_some()),
                "所有节点测速失败"
            );
        }
        Command::Switch { group, node } => {
            switch(&api, &runtime, group, node, interactive).await?
        }
        Command::MergePath => unreachable!(),
    }
    Ok(())
}
