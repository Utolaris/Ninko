# Changelog

## [1.0.0] — 2026-09-24

对齐 **Clash Verge（Rev）2.5.5** 数据目录、控制器与 sidecar socket 布局。

### 已实现功能

**交互菜单（中文，面向人类）**
- 不带参数进入菜单：切换节点 / 查看订阅 / Clash Verge / 退出
- 菜单底部直接显示全局扩展配置（Merge）完整路径
- 方向键选择、Enter 确定、Esc 返回；键位提示仅在主菜单顶部显示一次
- 当前光标与正在使用节点以薰衣草紫高亮；其他节点按测速状态着色
- 节点搜索：名称 / 全拼 / 拼音首字母包含匹配（如 `香港`、`xiang`、`xg`），忽略英文大小写；当前节点标记「当前使用」
- 订阅表格：全部节点、协议、使用它的策略组，不截断节点数量
- 进入策略组即并发测速，逐节点刷新，保持列表顺序与选择位置
- 延迟着色：&lt;200 ms 绿，200–400 ms 蓝，400–1000 ms 橙，&gt;1000 ms 或不通标红并显示「超时」
- Esc 直接退出当前选择，不等待未完成测速
- 切换 / 控制结果仅显示在对应界面，按任意键清除后回主菜单
- Clash Verge 控制：重启 / 退出桌面应用；仅作用当前用户；Linux 重启保留原应用路径（含 AppImage）与桌面会话环境

**Agent CLI（JSON，面向 LLM / 脚本）**
- stdout 始终为一个 JSON 对象；错误 `{"ok":false,"error":"..."}` 且退出码 1
- `ninko -h` / 子命令 `-h` 为英文
- `list`：当前 Profile、策略组、proxy-providers、内置节点（排除 Compatible 集合，不泄露订阅 URL / 密钥）
- `test`：并发 HTTP 延迟测速，输出 `passed` / `failed` 两组，并为节点分配稳定 `index`
- `switch <index>` / `switch <group> <index|节点名>`：按最近一次 `test` 的 index 或完整节点名切换，读回确认
- `control restart|stop`：重启 / 退出 Clash Verge 桌面应用，重启后等待控制器就绪
- `merge-path`：返回全局扩展 YAML 真实绝对路径
- index 缓存：Linux `~/.cache/ninko/last-test.json`（遵循 `XDG_CACHE_HOME`），macOS `~/Library/Caches/ninko/last-test.json`，可用 `NINKO_STATE_DIR` 覆盖

**连接与平台**
- 自动发现控制器：配置指定 socket → 数据目录 / 系统临时目录 `verge-mihomo.sock` → 旧版 `/tmp/verge/verge-mihomo.sock` → HTTP `external-controller` → `127.0.0.1:9090`
- 显式 `--api` / `--sock` / `--data-dir`，以及 `CLASH_API` / `CLASH_SOCK` / `CLASH_SECRET` / `CLASH_DATA_DIR`
- 数据目录对齐 Clash Verge Rev `dirs::data_dir()/APP_ID`：macOS Application Support、Linux XDG Data
- 支持 macOS 与 Linux；安装脚本注册 `ninko` / `Ninko` 并配置 PATH，无需 sudo
- 测速可调：`--concurrency`、`--timeout`、`--url`

**权限边界**
- Agent 说明中明确：未经用户明确授权，不得切换节点或停止 / 重启 Clash Verge

### 硬性前提

本机须安装并运行 Clash Verge（Rev）。Ninko 不打包、不启动、不替代代理核心。
