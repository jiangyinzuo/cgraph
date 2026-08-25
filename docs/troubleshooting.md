# 故障排查

## 搜索框显示分析 provider 不可用

先确认 server 在 `PATH` 中：

```bash
rust-analyzer --version
clangd --version
pyrefly --version
```

如果项目没有自动检测标志，显式传入：

```bash
cgraph --lsp rust-analyzer --workspace /path/to/project
# Python 默认命令；cgraph 会自动添加 `lsp` 子命令：
cgraph --lsp pyrefly --workspace /path/to/project
```

cgraph 会把 LSP 启动/initialize 或 Tree-sitter 初始化错误显示在状态栏和搜索框中。当前 server stderr 为了避免破坏 TUI 备用屏幕而被丢弃；更详细的日志输出仍是待实现功能。需要绕过损坏或缺失的 LSP 时，可以使用 `--no-lsp` 强制尝试 Tree-sitter 回退。

## 搜索一直为空

可能原因：

- 弹窗显示 `Waiting for typing pause…`，说明 200 ms 防抖尚未结束；持续产生键盘输入会不断重置计时。
- 弹窗显示 `Searching workspace symbols…`，说明请求已经开始，但 server 尚未完成当前 workspace symbol 查询。
- server 仍在索引大型工作区，当前查询返回不完整；稍后修改或重新输入查询可再次请求。
- 查询的符号类型被 `ac` / `at` 过滤；函数使用 `ac`，类型使用 `at`。
- 工作区根目录选择错误。
- 目标位于第三方依赖或工作区根目录之外；默认项目范围会主动排除它。
- workspace 根目录的 `.cgraph.toml` 匹配了该完整显示名；临时移除对应模式并重启可确认。
- server 对单次 workspace symbol 响应设有数量上限；输入更精确的查询可以缩小结果集。
- server 自身的 workspace symbol 能力未覆盖该语言或文件。
- Pyrefly 当前要求 workspace-symbol query 至少包含 3 个字符；更短输入会由 server 返回空结果。
- Tree-sitter 会跳过隐藏目录、`target`、`node_modules` 和符号链接；这些位置中的符号不会进入静态索引。

使用独立示例区分 TUI 过滤和 server 原始结果：

```bash
cargo run --example lsp_workspace_symbols -- rust-analyzer QueryText /path/to/project
```

## 展开按钮显示 `[!]`

`[!]` 表示该方向的 hierarchy 请求失败，不等于成功查询后没有孩子；成功空结果显示 `·`。选中该节点后，底部状态栏会显示错误文本，再按相同的 `tl` / `tr` 会重新请求。

常见原因包括 server 不支持对应的 call/type hierarchy 方法且当前语言没有 Tree-sitter 后备、CLI 名称根存在多个同名候选、语言服务器仍在索引，或者请求期间分析快照变化并返回 `content modified`。Tree-sitter 也会拒绝没有精确位置的同名根。同名根应改用 `ac` / `at` 选择精确位置；瞬时索引或 `content modified` 错误可以稍后重试。

rust-analyzer 当前不实现标准 type hierarchy；旧版 cgraph 或绕过 capability 检查的原始客户端会收到 `unknown request`。当前 `lsp_hierarchy ... type ...` 诊断示例会在发送请求前报告 server 未声明该能力，TUI 则保留 rust-analyzer 用于搜索与 call hierarchy，只把 type hierarchy 自动交给 Rust Tree-sitter 后备。看到 `syntactic relations only` 表示回退正常生效；这类结果只覆盖能从 `impl Trait for Type` 等项目语法唯一判定的关系。

可以绕过 TUI 直接诊断；文件、行和列参数是一基坐标：

```bash
cargo run --example lsp_hierarchy -- rust-analyzer call outgoing main /path/to/project /path/to/project/src/main.rs 1 4
```

## 底部状态栏长时间显示 Working

`Working` 来自语言服务器主动发送的 progress 或 server status，不是 cgraph 根据搜索耗时猜测的状态。大型 Rust 工作区首次启动时，Cargo metadata、crate graph、proc macro、构建检查和符号索引都可能持续较久；消息和百分比取决于 rust-analyzer 当前报告的阶段。

- 可以继续输入 `ac` / `at` 发起查询，但索引未完成时结果可能暂时不完整。
- `Ready` 只表示当前没有已知活动任务；不发送进度通知的 server 可能在后台工作但仍显示 Ready。
- `Warning` 或 `Error` 时先阅读状态后附带的消息；终端过窄时消息可能截断，可以扩大终端。目前 server stderr 尚未接入，请同时保留 server 版本和启动参数。
- `Disconnected` 表示 LSP 通道已经结束。当前版本不会自动重启 server；退出后重新运行 cgraph，并用显式 `--lsp` 检查是否可复现。
- `Tree-sitter: <language> · Ready` 表示 grammar 和 tags query 已初始化；第一次搜索或展开会惰性建立项目静态索引。展开后的 `syntactic relations only` 提示表示动态、歧义或项目外关系可能省略。
- `Backend: none · Inactive` 表示没有 LSP，且工作区未检测到 Rust、C、C++ 或 Python 的 Tree-sitter 标志。

快捷键和状态始终共用最底部一行，状态紧跟快捷键提示。终端较窄时整行从右侧自然截断，不代表快捷键或后端被关闭；扩大终端可看到完整状态摘要。

## 消息摘要与 pager

保存、配置、IPC、后备能力提示以及分析查询错误都会显示在倒数第二行，并进入统一消息历史；错误使用 `ERROR: <message>`。最底行的快捷键和分析状态不会被消息替换。按 `g<` 可从倒数第二行向上打开消息 pager；pager 最多占 15 行，支持 `j` / `k`、方向键、整页/半页滚动和 `g` / `G` 首尾跳转，`q` / `Esc` 关闭。

如果 `g<` 没有打开窗口，请先确认当前处于 Canvas 模式，而不是搜索、保存或帮助弹窗；这些弹窗会优先消费自己的按键。pager 中按 `V` 开始或取消按屏幕行选择，使用移动键扩展范围，再按 `y` 通过 OSC 52 复制原始文本。pager 打开期间 cgraph 也会关闭鼠标捕获，可直接拖拽选择并使用终端自己的复制快捷键；关闭后恢复 Canvas 鼠标操作。OSC 52 或鼠标复制是否可用取决于本地终端、SSH 和 tmux 配置。

## server 参数无法识别

以 `-` 开头的参数使用：

```bash
cgraph --lsp clangd --lsp-arg=--background-index
```

多个参数重复写 `--lsp-arg`，不要把完整 shell 命令放进 `--lsp`。

## 终端画面异常

正常退出会恢复 raw mode、鼠标捕获和备用屏幕。如果进程被强制终止，终端可能没有收到恢复序列，可以尝试：

```bash
reset
```

在部分 shell 中也可以运行 `stty sane`。如果问题能够通过普通 `q` 退出稳定复现，请记录终端类型、`TERM` 环境变量和最小操作步骤。

## `ec` 无法打开或应用配置

- cgraph 优先读取项目约定的 `$EDITER`，未设置时回退 `$EDITOR`。值必须是可直接执行的程序或路径，例如 `export EDITER=nvim`；两者都为空时 footer 会提示设置变量。
- 编辑器收到的目标始终是 `<workspace>/.cgraph.toml`。文件不存在时 cgraph 会先安全创建最小模板；workspace 不可写时保留 TUI 并显示创建错误。
- 编辑器非零退出时不重载配置。检查编辑器自己的退出状态，不要把带参数的完整 shell 命令写进变量。
- TOML 无效、字段拼错、类型错误或包含空过滤模式时，倒数第二行显示 `Project config reload failed` 并进入消息历史；上一份有效规则和图保持不变。修正后再次执行 `ec`。
- 成功后倒数第二行显示正在刷新的分支数量。刷新异步进行，期间旧关系继续可见；新结果到达后才按新规则替换。没有分析 provider 时规则已重载，但无法重新查询现有关系。

## 双击节点没有跳转编辑器

- 启动命令必须包含 `--ipc-socket <PATH>`，且 socket 的父目录已经存在、不是符号链接、没有 group/other 写权限；推荐使用权限为 `0700` 的 `$XDG_RUNTIME_DIR/cgraph`。
- 编辑器客户端必须连接到完全相同的路径。底栏显示 `no IPC editor client is connected` 表示当前连接数为零。
- provisional CLI 根或 provider 没有返回完整 URI、行、列时，底栏显示 `Selected node has no exact source location`，不会发送伪造位置；可用 `ac` / `at` 选择精确结果。
- 已有普通文件、符号链接、活跃 socket 或没有匹配 ownership marker 的 socket 时，cgraph 会拒绝启动而不是覆盖。确认路径无误后人工处理该文件，不要用递归删除命令清理 runtime 目录。
- 客户端必须逐行解析 JSON，检查 `version == 1`，把零基行号加一，并把 UTF-16 character 转换为编辑器所需的列格式。完整示例见[编辑器联动](editor-integration.md)。

## 编辑器请求没有聚焦节点

- 请求必须是一行完整 JSON，以换行结束，使用协议版本 `1`，并携带非空整数 `request_id`。响应使用相同 id；错误发生在 envelope 可解析之前时 id 为 `null`。
- `hierarchy` 只能是 `call` 或 `type`，符号名不能为空。提供 `location` 时，必须同时包含非空 `file://` URI、零基 line 和零基 UTF-16 character。
- 不带位置的请求只在同 kind 同名节点唯一时复用。底栏或响应出现 `ambiguous` 时，应从编辑器补充精确位置，而不是猜测任意候选。
- `accepted` 表示节点已经固定、选中并居中，不表示 hierarchy 已经加载；仍需在 cgraph 中使用 `tl` / `tr`。
- `unsupported IPC protocol version` 表示客户端与 cgraph 协议不兼容。不要忽略后继续解释 payload；升级对应一侧或使用匹配版本。
- `IPC frame exceeds 1 MiB` 后连接会关闭。`focus_symbol` 本应很小；出现该错误通常是客户端 framing 错误或意外把多条消息拼成一帧。

## 消息过大错误

cgraph 拒绝超过 16 MiB 的单条 LSP 消息，以避免无界内存分配。遇到此错误通常意味着 server 返回异常大的 workspace symbol 集合；缩小工作区或查询范围，并保留 server 版本信息用于报告问题。这个限制与编辑器 IPC 的 1 MiB NDJSON 入站帧限制不同。

## 报告问题时建议附带

- cgraph 版本或 commit。
- 操作系统、终端和 `TERM`。
- 语言服务器名称及版本。
- 启动命令和工作区类型。
- 能否通过 `lsp_workspace_symbols` 或 `lsp_hierarchy` 示例复现。
- 错误文本；不要附带私有源码内容。
