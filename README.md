# Ninko

**人狐**（にんこ）—— 日语里狐狸成精的名字。Clash Verge 的 logo 是狐狸，Ninko 是站在它旁边的终端人形：没有 GUI 时，由人在命令行里替它拿主意。

一个面向人类的轻量化 Clash CLI。

## 设计初衷

Linux 工作站、SSH 进去的远程主机、没有桌面环境的服务器——这些场景里装着 Clash Verge，却几乎没法点它的图形界面。切个节点、看一眼订阅、确认全局配置路径，本该是敲两下键盘的事，不该为了一个下拉菜单去开远程桌面。

Ninko 就是为这件事存在的：在终端里快速调节 Clash 节点。

**硬性前提：本机必须安装并运行 Clash Verge（Rev）。** Ninko 不打包、不启动、不替代代理核心，只通过已运行的 Clash Verge 控制接口读写节点。没有 Clash Verge 就没有可调的对象。

## 开始使用

```sh
./scripts/install.sh
ninko # 或 Ninko
```

不带参数进入交互菜单：**切换节点 / 查看订阅 / 退出**。菜单底部直接显示全局配置的完整路径，无需进入二级页面。

- 用方向键选择，Enter 确定，Esc 返回。
- 当前光标选中项、正在使用的节点始终以薰衣草紫高亮；光标移开也不会改变正在使用节点的颜色。其他节点使用测速状态颜色。
- 搜索按名称、全拼、拼音首字母做包含匹配：`香港`、`xiang`、`xianggang`、`xg` 都能找到所有包含「香港」的节点，不区分英文大小写；当前节点会标记「当前使用」。
- 订阅按表格展示全部节点、协议、使用它的策略组，不截断节点数量。
- 进入策略组即并发测速，每个节点完成后立即刷新，保持列表顺序和选择位置。
- 节点延迟小于 200 ms 为绿色，200–400 ms 为蓝色，大于 400 ms 为橙色；不通的节点标红并显示「超时」。
- Esc 直接退出当前选择，不显示取消提示，不等待未完成的测速。
- 切换成功或失败的提示只出现在切换界面内，按任意键清除后才回到主菜单。

也可以直接运行 `cargo run --`，或使用编译后的 `./target/release/ninko`。

## 快捷命令

```sh
ninko list                       # 当前订阅和完整节点
ninko switch                     # 搜索选择策略组和节点
ninko switch 'AI 专用'           # 直接进入该组的节点选择
ninko switch '其他' '节点完整名称' # 直接切换，读回确认结果
ninko test                       # 可选快捷命令：批量测速；交互菜单已移除独立测速页
ninko test --provider trumpyun    # 只测一个订阅
ninko test --group 'AI 专用'      # 只测一个策略组，展开嵌套分组并去重
ninko merge-path                 # 仅输出全局扩展 YAML 的绝对路径
```

测速指 HTTP 请求延迟，单位毫秒，不是下载速度。默认并发 8、超时 5 秒；可设置 `--concurrency 4 --timeout 8000 --url https://www.gstatic.com/generate_204`。测速不发送节点切换请求；核心自身的自动选择策略仍按原配置工作。全部失败时命令以非零状态退出，部分失败保留每个节点的结果。

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

支持 Linux 和 macOS。Linux 遵循 `XDG_DATA_HOME`（默认 `~/.local/share`），也检查当前用户运行中的 `verge-mihomo -d` 目录及 `XDG_CONFIG_HOME`。AppImage、便携版或其他自定义路径可以使用 `--data-dir` 指定。需要 Clash Verge 已运行，且当前用户有权限访问控制 socket。

远程主机上典型用法：SSH 登录后直接 `ninko` 进菜单，或 `ninko switch` 搜索切换；不必为了改节点回到带桌面的机器。

Linux 安装（需要 Rust 工具链和 C 编译器）：

```sh
./scripts/install.sh
source ~/.zshrc
ninko
Ninko
```

安装器放置二进制到 `~/.local/bin`，注册大小写两种名称，并在需要时把该目录加入 zsh 的 PATH；无需 sudo。Linux 还可使用预编译的 x86_64 musl 静态二进制，通过 `./scripts/install.sh /路径/ninko` 安装。macOS 二进制不能复制到 Linux 使用。

Linux 二进制构建产物位于 `target/x86_64-unknown-linux-musl/release/ninko`。已在 Kali Linux x86_64 上运行测试与终端交互验证，并读取该主机真实 Clash Verge 的订阅和 Merge 路径；未切换真实节点。其他 Linux 发行版的打包目录仍可通过上述参数覆盖。

Linux 目录和 socket 兼容依据 [Clash Verge Rev 官方目录实现](https://github.com/clash-verge-rev/clash-verge-rev/blob/dev/src-tauri/src/utils/dirs.rs)。

`list --json`、`test --json` 可用于导出结果；重定向输出时不显示交互进度和颜色。

## 开发与验证

```sh
cargo fmt --check
cargo test
uv run python tests/terminal.py
cargo clippy --all-targets -- -D warnings
cargo build --release
```

测试使用本地模拟控制器，覆盖订阅过滤、中文及特殊字符路径、节点切换与读回、无效切换拦截、测速失败以及 Merge 路径定位，不切换真实节点。

控制接口依据 [mihomo 官方 API 文档](https://wiki.metacubex.one/api/)。
