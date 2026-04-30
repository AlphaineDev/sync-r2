# syncr2

[English](README.md) | [中文](README.zh-CN.md)

`syncr2` 是一个用 Rust 写的终端优先 Cloudflare R2 同步工具。

它会监听一个本地目录，把新增或修改的文件上传到 Cloudflare R2 bucket，并用本地 SQLite 数据库记录同步状态。日常操作通过 TUI 完成。当前项目不再包含浏览器前端、本地 HTTP API 或 WebSocket server。

## 当前形态

- 纯 Rust 运行时，使用 Ratatui 构建终端 UI。
- 通过 S3 兼容凭证访问 Cloudflare R2。
- 本地同步状态保存在 `data/syncr2.db`。
- 运行日志写入 `logs/`。
- 支持配置本地监听目录、R2 bucket、容量上限、过滤规则和上传并发。
- 没有浏览器前端，没有本地 API 端口，也没有 WebSocket server。

## 运行要求

- Rust 工具链。
- 能访问 Cloudflare R2 的网络环境。
- 一个 Cloudflare R2 bucket。
- 拥有目标 bucket 访问权限的 R2 API 凭证。

不需要 Cloudflare CLI 登录。程序只读取 `.env` 和 `config/default.toml` 里的配置。

## 快速开始

在项目目录创建 `.env`：

```env
R2_ACCESS_KEY_ID=your_access_key_id
R2_SECRET_ACCESS_KEY=your_secret_access_key
R2_ENDPOINT=https://your_account_id.r2.cloudflarestorage.com
R2_BUCKET_NAME=your_bucket_name
```

然后编辑 `config/default.toml`：

```toml
watch_path = "/path/to/local/folder"

[r2]
access_key_id = "${R2_ACCESS_KEY_ID}"
secret_access_key = "${R2_SECRET_ACCESS_KEY}"
endpoint = "${R2_ENDPOINT}"
bucket_name = "${R2_BUCKET_NAME}"
```

`watch_path` 支持绝对路径、`~/...`、`$HOME/...` 和 `${HOME}/...`。

启动 TUI：

```bash
cargo run
```

进入 Dashboard 后按 `s` 开始同步。

## 命令

```bash
cargo run
```

打开 TUI。

```bash
cargo run -- tui
```

显式打开 TUI。

```bash
cargo run -- sync start
cargo run -- sync stop
cargo run -- sync pause
cargo run -- sync resume
cargo run -- sync status
```

从 CLI 控制或查看同步引擎状态。

```bash
cargo run -- capacity
```

打印最近一次已知容量快照。

```bash
cargo run -- files
```

浏览配置的本地监听目录。

```bash
cargo run -- config show
```

打印不包含密钥字段的公开配置。

```bash
cargo run -- config migrate --from config.yaml --to config/default.toml
```

把旧版 YAML 配置迁移为当前 TOML 格式。

## TUI

界面包含五个主要页面：

- `Dashboard`：同步状态、队列统计、监听目录和容量概览。
- `File Browser`：本地和 R2 双栏文件浏览与文件操作。
- `Config Center`：在终端里编辑部分运行配置。
- `Capacity`：查看并校准 R2 使用量。
- `Sync Logs`：查看当前进程内的同步事件。

常用快捷键：

- `Tab`：切换到下一个主页面。
- `Shift+Tab`：切换到上一个主页面。
- `s`：在 Dashboard 启动同步。
- `x`：在 Dashboard 停止同步。
- `p`：在 Dashboard 暂停同步。
- `r`：在 Dashboard 恢复同步。
- `c`：在 Capacity 页面校准容量。
- `q`：退出。

文件浏览快捷键：

- `Enter`：进入选中的目录，或进入 `..` 返回上级目录。
- `Left` / `Right`：切换本地面板和 R2 面板。
- `u`：根据当前面板执行上传或下载。
- `d`：删除选中的项目。
- `[`：执行本地到云端的镜像同步。
- `]`：执行云端到本地的镜像同步。
- `y` / `n`：确认或取消危险操作。

配置页面快捷键：

- `Up` / `Down`：选择配置项。
- `Left` / `Right`：调整数值配置。
- `Enter`：编辑文本配置。

## 配置

`config/default.toml` 是主运行配置。

重要字段：

- `watch_path`：要同步的本地目录。
- `[r2]`：Cloudflare R2 凭证和 bucket 目标。
- `[capacity]`：允许使用的最大 R2 容量，单位是字节。
- `[watcher]`：include/exclude 过滤规则。
- `[concurrency]`：上传并发和批处理间隔。
- `[tui]`：TUI 刷新间隔和事件日志数量。
- `[logging]`：日志文件和轮转设置。

密钥应该保存在 `.env`。建议在 `config/default.toml` 里保留 `${R2_ACCESS_KEY_ID}` 这种占位符，避免把真实凭证写入可提交配置。

## 运行时文件

程序会创建这些本地运行时文件：

- `data/syncr2.db`：SQLite 同步状态库。
- `logs/sync.log`：运行日志。
- `target/`：Rust 构建产物。

这些路径已由 `.gitignore` 忽略。

## R2 说明

R2 通过 AWS S3 SDK 访问，使用配置中的 endpoint 和静态凭证。

如果同步没有连到 R2，优先检查：

- `.env` 是否位于项目目录。
- `R2_BUCKET_NAME` 是否和 Cloudflare 里的 bucket 名完全一致。
- access key 是否拥有目标 bucket 的对象读写权限。
- `R2_ENDPOINT` 是否是账号级 R2 S3 endpoint。
- `watch_path` 是否存在，并且其中的文件没有被 include/exclude 规则过滤掉。

Dashboard 上显示 `Stopped` 时，通常还没有发生任何 R2 请求。R2 连接是懒加载的：同步、R2 浏览、上传/下载或容量校准需要时才会创建连接。

## 同步语义

自动同步目前是“本地新增/修改 -> 上传到 R2”。本地删除文件不会自动删除 R2 对应对象。

启动同步时会扫描整个 `watch_path`，已有文件会进入 pending 队列；处理每个文件时会比较远端 metadata 中的 `sha256`，内容一致则标记为 skipped，不会重复上传。

R2 没有真实文件夹对象，目录效果来自对象 key 里的 `/` 前缀。TUI 会把这些前缀折叠成文件夹显示，并支持 `Enter` 进入和 `..` 返回。

## 安全

不要提交 `.env` 或真实 R2 凭证。如果凭证被贴到日志、聊天、截图或误提交到仓库里，请在 Cloudflare 里轮换凭证，并更新 `.env`。

## 开发

构建检查：

```bash
cargo check
```

运行测试：

```bash
cargo test
```

格式化：

```bash
cargo fmt
```
