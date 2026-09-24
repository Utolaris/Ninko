# Ninko

Ninko 是一种会附身于人的狐灵，由于 Clash Verge 的 logo 是狐狸，而此 CLI 同时面向人类和 AI 设计，因此而得名。

一个面向人类和 AI 的轻量化 Clash CLI。

**版本 1.0.0** · 对齐 Clash Verge（Rev）**2.5.5**

## 设计初衷

你的远程主机没有图形化界面，而恰好你的 VPN 崩了，需要切换节点，Ninko 针对这种场景设计。

**硬性前提：本机必须安装并运行 Clash Verge（Rev）。** Ninko 不打包、不启动、不替代代理核心，只通过已运行的 Clash Verge 控制接口读写节点。没有 Clash Verge 就没有可调的对象。

## 开始使用

**人类**：不带参数进入中文交互菜单。**Agent / LLM**：子命令输出 JSON，`ninko -h` 为英文。

### Homebrew 安装

macOS 与 Linux（Linuxbrew）均可。本仓库即 tap 源，formula 在 [`Formula/ninko.rb`](Formula/ninko.rb)，从源码构建（依赖 `rust`，brew 会自动装）。

因仓库名是 `Ninko` 而非 `homebrew-ninko`，**必须写全 URL**；`brew tap Utolaris/Ninko` 会去找 `homebrew-Ninko`，会失败。

```sh
brew tap utolaris/ninko https://github.com/Utolaris/Ninko.git
brew install ninko
ninko --version   # ninko 1.0.0
```

已在 Kali Linux（Homebrew 7 / Linuxbrew）上实测：`brew tap` → `brew install` → `ninko --version` 通过。

### 源码安装

```sh
./scripts/install.sh
ninko # 交互菜单（中文）
ninko --help # Agent 命令（英文 + JSON）
```

不带参数进入交互菜单：**切换节点 / 查看订阅 / Clash Verge / 退出**。菜单底部直接显示全局配置的完整路径，无需进入二级页面。

- 用方向键选择，Enter 确定，Esc 返回。**键位提示只在主菜单顶部显示一次**，子界面不再重复。
- **Clash Verge** 可重启或退出桌面应用；仅控制当前用户的桌面应用；退出失败会报错。Linux 重启保留原应用路径（包括 AppImage）和桌面会话环境。
- 当前光标选中项、正在使用的节点始终以薰衣草紫高亮；光标移开也不会改变正在使用节点的颜色。其他节点使用测速状态颜色。
- 搜索按名称、全拼、拼音首字母做包含匹配：`香港`、`xiang`、`xianggang`、`xg` 都能找到所有包含「香港」的节点，不区分英文大小写；当前节点会标记「当前使用」。
- 订阅按表格展示全部节点、协议、使用它的策略组，不截断节点数量。
- 进入策略组即并发测速，每个节点完成后立即刷新，保持列表顺序和选择位置。
- 节点延迟小于 200 ms 为绿色，200–400 ms 为蓝色，大于 400 ms 且不超过 1000 ms 为橙色；超过 1000 ms 或不通的节点标红并显示「超时」，JSON 归入 `failed`。
- Esc 直接退出当前选择，不显示取消提示，不等待未完成的测速。
- 切换 / 控制 的结果只出现在对应界面内，按任意键清除后才回到主菜单。

也可以直接运行 `cargo run --`，或使用编译后的 `./target/release/ninko`。

## Agent CLI（JSON）

子命令面向 LLM / 脚本：stdout 始终是**一个 JSON 对象**，错误为 `{"ok":false,"error":"..."}` 且退出码 1。`ninko -h` / `ninko <cmd> -h` 为英文。

```sh
ninko list                       # {ok, current_profile, groups, providers, inline_nodes}
ninko test                       # {ok, passed:[{index,provider,name,delay_ms,group}], failed:[...]}
ninko test --group 'AI 专用'
ninko switch 3                   # 用 test 结果里的 index 切换（推荐 LLM 流程）
ninko switch 'AI 专用' 3         # 指定策略组 + index
ninko switch '其他' '节点完整名称' # 仍可用完整节点名
ninko control restart|stop       # 重启 / 退出 Clash Verge 桌面应用
ninko merge-path                 # {ok, path}
```

**Agent / LLM 推荐流程**：先 `ninko test`（每个节点带 `index`），再 `ninko switch <index>`。index 缓存在系统缓存目录：Linux 为 `~/.cache/ninko/last-test.json`（遵循 `XDG_CACHE_HOME`），macOS 为 `~/Library/Caches/ninko/last-test.json`；可用 `NINKO_STATE_DIR` 覆盖。

`test` 不输出表格：完成后自动分成 **passed / failed** 两组 JSON。部分失败仍退出 0（两组都有数据）；完全没有可测节点才退出 1。


测速指 HTTP 请求延迟，单位毫秒，不是下载速度。默认并发 8、超时 5 秒；可设置 `--concurrency 4 --timeout 8000 --url https://www.gstatic.com/generate_204`。测速不发送节点切换请求；核心自身的自动选择策略仍按原配置工作。

`list` 展示运行核心已加载的 `proxy-providers`，并附带当前 Profile 信息。排除核心自动生成的 Compatible 集合，避免把策略组当成订阅、重复列出节点；配置内置节点单独展示。“已加载”不代表全部节点当前都有连接。输出不包含订阅 URL、密码或节点密钥。

## 全局配置

`merge-path` 从 Clash Verge 的 `profiles.yaml` 查找 `uid: Merge` 的文件，返回真实绝对路径，不会误选单订阅增强文件、备份或核心生成的配置。应用关闭时也可以查询路径。

本机对应：

```text
~/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev/profiles/Merge.yaml
```

这是文件开头含「设计：节点通过 proxy-providers 从订阅拉取」「仅两个策略组：AI 专用 / 其他」的全局扩展配置。CLI 只暴露路径，不修改或重载配置。

## 连接设置

自动读取 Clash Verge 运行配置中的控制器地址、Unix socket 和密钥。优先连接可用的 socket：配置指定的位置、数据目录下的 `verge-mihomo.sock`、系统临时目录，以及旧版 `/tmp/verge/verge-mihomo.sock`；没有可用 socket 时使用配置中的 HTTP 控制器，配置也未指定才使用 `127.0.0.1:9090`。显式 `--api` 优先于自动发现，同时指定 `--sock` 时以显式 socket 为准。显式 HTTP 地址不会自动附带本机配置中的密钥。

```sh
ninko --data-dir '/自定义/Clash Verge 数据目录' list
ninko --api http://127.0.0.1:9090 list
CLASH_SECRET='控制器密钥' ninko --api http://127.0.0.1:9090 test
```

可用环境变量：`CLASH_DATA_DIR`、`CLASH_SOCK`、`CLASH_API`、`CLASH_SECRET`。默认数据目录使用系统应用数据目录下的 `io.github.clash-verge-rev.clash-verge-rev`；其他安装版本可显式传入 `--data-dir`。

## Linux 与远程主机

支持 Linux 和 macOS，数据目录对齐 Clash Verge Rev 的 `dirs::data_dir()/APP_ID`：

| 平台 | 数据目录 | Sidecar socket |
| --- | --- | --- |
| macOS | `~/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev` | 用户临时目录（`$TMPDIR`）下 `verge-mihomo.sock` |
| Linux | `$XDG_DATA_HOME/io.github.clash-verge-rev.clash-verge-rev`（默认 `~/.local/share/...`） | 数据目录下 `verge-mihomo.sock` |

也会检查 `XDG_CONFIG_HOME` / `dirs::config_dir()` 下的同名目录，以及 Linux 上当前用户运行中的 `verge-mihomo -d` 目录。AppImage、便携版或其他自定义路径使用 `--data-dir` 指定；路径不存在会直接报「数据目录不存在」。未安装 Clash Verge 时，找不到 `profiles.yaml` 会提示先安装并运行过 Clash Verge Rev；控制器连不上会提示确认应用已运行。需要当前用户有权限访问控制 socket。

远程主机上典型用法：SSH 登录后直接 `ninko` 进菜单，或 `ninko switch` / `ninko test`；不必为了改节点回到带桌面的机器。

Linux 安装（需要 Rust 工具链和 C 编译器）：

```sh
./scripts/install.sh
export PATH="$HOME/.local/bin:$PATH"
ninko
Ninko
```

安装器放置二进制到 `~/.local/bin`，注册大小写两种名称，并根据 `SHELL` 配置 zsh、bash 或 POSIX 登录 shell 的 PATH；zsh 遵循 `ZDOTDIR`，bash 同时配置交互与登录启动文件。无需 sudo。Linux 还可使用预编译的 x86_64 musl 静态二进制，通过 `./scripts/install.sh /路径/ninko` 安装。macOS 二进制不能复制到 Linux 使用。

Linux 二进制构建产物位于 `target/x86_64-unknown-linux-musl/release/ninko`。已在 Kali Linux x86_64 上运行测试与终端交互验证，并读取该主机真实 Clash Verge 的订阅和 Merge 路径；未切换真实节点。其他 Linux 发行版的打包目录仍可通过上述参数覆盖。

Linux 目录和 socket 兼容依据 [Clash Verge Rev 官方目录实现](https://github.com/clash-verge-rev/clash-verge-rev/blob/dev/src-tauri/src/utils/dirs.rs)。

`list --json`、`test --json` 可用于导出结果；重定向输出时不显示交互进度和颜色。

## 开发与验证

```sh
cargo fmt --check
cargo test
uv run python tests/terminal.py
sh tests/install.sh "$PWD/target/debug/ninko" "$PWD/scripts/install.sh"
cargo clippy --all-targets -- -D warnings
cargo build --release
```

测试使用本地模拟控制器，覆盖订阅过滤、中文及特殊字符路径、节点切换与读回、无效切换拦截、测速失败以及 Merge 路径定位，不切换真实节点。

控制接口依据 [mihomo 官方 API 文档](https://wiki.metacubex.one/api/)。
