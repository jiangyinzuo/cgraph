# cgraph

`cgraph` 是一个用于浏览函数调用层次与类型继承层次的交互式 TUI 工具。目前已经具备可拖拽的有向图无限画布、全局节点去重、循环/反向边标记、鼠标与空间键盘导航、LSP 语义查询、Tree-sitter 项目静态搜索与 hierarchy 回退、入口/分支管理、文本导出、底部分析状态栏，以及与 Neovim 等客户端双向联动的 Unix socket IPC。

## 安装

已发布到 crates.io 后，可以使用 Cargo 安装：

```bash
cargo install call-graph-cli
cgraph
```

## 快速开始

```bash
# 打开空画布；会根据当前工作区尝试启动语言服务器
cargo run

# 创建带初始 anchor 的画布
cargo run -- call Foo::Bar
cargo run -- type Student

# 显式指定语言服务器和工作区
cargo run -- --lsp rust-analyzer --workspace /path/to/project

# 不启动 LSP，使用 Tree-sitter 静态项目索引
cargo run -- --no-lsp --workspace /path/to/project

# 在调用方创建的私有 runtime 目录中启用编辑器双向 IPC
install -d -m 700 "$XDG_RUNTIME_DIR/cgraph"
cargo run -- --ipc-socket "$XDG_RUNTIME_DIR/cgraph/project.sock"
```

进入 TUI 后，输入 `ac` 搜索 call 节点，输入 `at` 搜索 type 节点；`tl` / `tr` 按需加载并展开左/右关系，拖拽节点或画布空白处查看屏幕外内容，`ec` 编辑并重载项目配置，`?` 查看完整操作帮助，`q` 退出。同一语义符号在整个图中只显示一次；循环或无法保持从左向右的边使用黄色双线和特殊符号。搜索弹窗会异步加载当前项目的符号，随后在本地即时模糊筛选；默认不包含第三方依赖。完整用法参见 [`docs/`](docs/README.md)。

项目根目录可以添加 `.cgraph.toml`，用完整限定名和 `*` 通配符过滤高噪声符号；本仓库的配置示例会过滤任意类的 `into`、`is_some` 和 `Some`。详见 [`docs/project-configuration.md`](docs/project-configuration.md)。

## LSP 配置

`cgraph` 会从工作区的项目文件推断 `rust-analyzer`、`clangd` 或 Pyrefly；Python 默认启动 `pyrefly lsp`。自动检测不适合复杂的多语言仓库时，可以使用：

```bash
cgraph --lsp clangd --lsp-arg=--background-index --workspace /path/to/project
cgraph --no-lsp
```

运行 `cgraph --help` 查看全部命令行选项。独立调试 workspace symbol 或 hierarchy 查询时，可以运行：

```bash
cargo run --example lsp_workspace_symbols -- rust-analyzer LspProvider .
cargo run --example lsp_hierarchy -- rust-analyzer call outgoing main . src/main.rs 16 10
```

## 文档导航

- [`docs/README.md`](docs/README.md)：用户文档入口。
- [`docs/getting-started.md`](docs/getting-started.md)：安装和第一次使用。
- [`docs/commands.md`](docs/commands.md)：命令行、按键和鼠标操作。
- [`docs/project-configuration.md`](docs/project-configuration.md)：项目本地符号过滤和通配规则。
- [`docs/editor-integration.md`](docs/editor-integration.md)：Unix socket 与 Neovim 双向联动。
- [`docs/language-servers.md`](docs/language-servers.md)：语言服务器选择与配置。
- [`docs/troubleshooting.md`](docs/troubleshooting.md)：常见问题排查。
- [`DESIGN.md`](DESIGN.md)：产品愿景、范围和需求导航。
- [`requirements/README.md`](requirements/README.md)：父子需求、状态、优先级和验收条件。
- [`src/README.md`](src/README.md)：源代码架构、依赖方向和开发约束。
- [`src/state/README.md`](src/state/README.md)：节点身份、图状态、刷新与缓存语义。
- [`src/fetch/README.md`](src/fetch/README.md)：LSP/Tree-sitter 查询层和并发模型。
- [`src/tui/README.md`](src/tui/README.md)：事件循环、搜索弹窗和未来画布设计。
- [`src/testing/README.md`](src/testing/README.md)：测试分层、自动化清单、维护规则和当前缺口。
- [`src/ipc/README.md`](src/ipc/README.md)：Unix socket 生命周期、广播协议和安全边界。

## 开发检查

```bash
cargo fmt --all --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```

新增、删除或移动测试时必须同步更新 [`src/testing/README.md`](src/testing/README.md) 的清单和覆盖说明；测试套件会自动校验逐文件数量。
