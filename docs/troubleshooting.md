# 故障排查

## 搜索框显示 LSP 不可用

先确认 server 在 `PATH` 中：

```bash
rust-analyzer --version
clangd --version
pylsp --version
```

如果项目没有自动检测标志，显式传入：

```bash
ctree --lsp rust-analyzer --workspace /path/to/project
```

ctree 会把启动或 initialize 错误显示在搜索框状态行。当前 server stderr 为了避免破坏 TUI 备用屏幕而被丢弃；更详细的日志输出仍是待实现功能。

## 搜索一直为空

可能原因：

- 弹窗显示 `Waiting for typing pause…`，说明 200 ms 防抖尚未结束；持续产生键盘输入会不断重置计时。
- 弹窗显示 `Searching workspace symbols…`，说明请求已经开始，但 server 尚未完成当前 workspace symbol 查询。
- server 仍在索引大型工作区，当前查询返回不完整；稍后修改或重新输入查询可再次请求。
- 查询的符号类型被 `ac` / `at` 过滤；函数使用 `ac`，类型使用 `at`。
- 工作区根目录选择错误。
- 目标位于第三方依赖或工作区根目录之外；默认项目范围会主动排除它。
- workspace 根目录的 `.ctree.toml` 匹配了该完整显示名；临时移除对应模式并重启可确认。
- server 对单次 workspace symbol 响应设有数量上限；输入更精确的查询可以缩小结果集。
- server 自身的 workspace symbol 能力未覆盖该语言或文件。

使用独立示例区分 TUI 过滤和 server 原始结果：

```bash
cargo run --example lsp_workspace_symbols -- rust-analyzer QueryText /path/to/project
```

## 展开按钮显示 `[!]`

`[!]` 表示该方向的 hierarchy 请求失败，不等于成功查询后没有孩子；成功空结果显示 `·`。选中该节点后，底部状态栏会显示错误文本，再按相同的 `tl` / `tr` 会重新请求。

常见原因包括 server 不支持对应的 call/type hierarchy 方法、CLI 名称根存在多个同名候选、语言服务器仍在索引，或者请求期间分析快照变化并返回 `content modified`。同名根应改用 `ac` / `at` 选择精确位置；瞬时索引或 `content modified` 错误可以稍后重试。

可以绕过 TUI 直接诊断；文件、行和列参数是一基坐标：

```bash
cargo run --example lsp_hierarchy -- rust-analyzer call outgoing main /path/to/project /path/to/project/src/main.rs 1 4
```

## 底部状态栏长时间显示 Working

`Working` 来自语言服务器主动发送的 progress 或 server status，不是 ctree 根据搜索耗时猜测的状态。大型 Rust 工作区首次启动时，Cargo metadata、crate graph、proc macro、构建检查和符号索引都可能持续较久；消息和百分比取决于 rust-analyzer 当前报告的阶段。

- 可以继续输入 `ac` / `at` 发起查询，但索引未完成时结果可能暂时不完整。
- `Ready` 只表示当前没有已知活动任务；不发送进度通知的 server 可能在后台工作但仍显示 Ready。
- `Warning` 或 `Error` 时先阅读状态后附带的消息；终端过窄时消息可能截断，可以扩大终端。目前 server stderr 尚未接入，请同时保留 server 版本和启动参数。
- `Disconnected` 表示 LSP 通道已经结束。当前版本不会自动重启 server；退出后重新运行 ctree，并用显式 `--lsp` 检查是否可复现。
- `Tree-sitter: <language> · Ready` 表示 grammar 和 tags query 已初始化；`ac` / `at` 的 workspace symbol 搜索目前仍需要 LSP。
- `Backend: none · Inactive` 表示没有 LSP，且工作区未检测到 Rust、C、C++ 或 Python 的 Tree-sitter 标志。

快捷键和状态始终共用最底部一行。终端较窄时两侧文字各自在分配区域中截断，不代表快捷键或后端被关闭。

## server 参数无法识别

以 `-` 开头的参数使用：

```bash
ctree --lsp clangd --lsp-arg=--background-index
```

多个参数重复写 `--lsp-arg`，不要把完整 shell 命令放进 `--lsp`。

## 终端画面异常

正常退出会恢复 raw mode、鼠标捕获和备用屏幕。如果进程被强制终止，终端可能没有收到恢复序列，可以尝试：

```bash
reset
```

在部分 shell 中也可以运行 `stty sane`。如果问题能够通过普通 `q` 退出稳定复现，请记录终端类型、`TERM` 环境变量和最小操作步骤。

## 消息过大错误

ctree 拒绝超过 16 MiB 的单条 LSP 消息，以避免无界内存分配。遇到此错误通常意味着 server 返回异常大的 workspace symbol 集合；缩小工作区或查询范围，并保留 server 版本信息用于报告问题。

## 报告问题时建议附带

- ctree 版本或 commit。
- 操作系统、终端和 `TERM`。
- 语言服务器名称及版本。
- 启动命令和工作区类型。
- 能否通过 `lsp_workspace_symbols` 或 `lsp_hierarchy` 示例复现。
- 错误文本；不要附带私有源码内容。
