# cgraph 源代码设计

本文档面向维护者，描述 `src/` 内部边界。产品愿景位于仓库根目录 `DESIGN.md`，用户可见行为以[需求树](../requirements/README.md)中对应需求为准；当实现和产品需求不一致时，应先记录差异，再决定修改代码还是需求文档。

## 需求追踪

内部设计不重复定义用户行为。主要模块与父需求的关系如下：

| 模块 | 主要需求 |
| --- | --- |
| `main.rs` / `cli.rs` | [REQ-1 会话与启动](../requirements/REQ-1-session/README.md)、[REQ-8 语言支持](../requirements/REQ-8-language-support/README.md) |
| `config/` | [REQ-9 项目本地配置与符号过滤](../requirements/REQ-9-project-configuration/README.md) |
| `app.rs` / `state/` | [REQ-3 层次关系探索](../requirements/REQ-3-hierarchy/README.md)、[REQ-4 画布与导航](../requirements/REQ-4-canvas-navigation/README.md)、[REQ-5 符号与图入口管理](../requirements/REQ-5-symbol-management/README.md) |
| `fetch/` | [REQ-2 分析后端状态](../requirements/REQ-2-analysis-status/README.md)、[REQ-3 层次关系探索](../requirements/REQ-3-hierarchy/README.md)、[REQ-5 符号与图入口管理](../requirements/REQ-5-symbol-management/README.md) |
| `tui/` | [REQ-2 分析后端状态](../requirements/REQ-2-analysis-status/README.md)、[REQ-4 画布与导航](../requirements/REQ-4-canvas-navigation/README.md)、[REQ-5 符号与图入口管理](../requirements/REQ-5-symbol-management/README.md) |
| `ipc/` | [REQ-6 进程间通信](../requirements/REQ-6-ipc/README.md) |
| `export/` | [REQ-7 导出关系图](../requirements/REQ-7-export/README.md) |

测试分层、逐文件清单、回归用例规则和尚未覆盖的测试类型统一维护在 [测试设计与覆盖总账](testing/README.md)。任何测试增删、移动或类型变化都必须同步刷新该文档；`tests/test_documentation.rs` 会自动检查逐文件数量，防止清单静默过期。

## 模块职责

| 模块 | 当前职责 | 不应承担的职责 |
| --- | --- | --- |
| `main.rs` | 组装 CLI、语言服务器、App 和终端生命周期 | 保存交互状态、解析 LSP 消息 |
| `cli.rs` | 命令行语法和启动配置 | 自动修改 App、启动外部进程 |
| `config/` | 读取并校验 workspace 根目录的 `.cgraph.toml` | 查询 LSP、修改关系图或渲染错误弹窗 |
| `app.rs` / `app/` | UI 无关的交互状态组合；`search`、`hierarchy`、`analysis`、`messages`、`config`、`save` 分别维护各自状态迁移 | 直接读终端、直接发送 JSON-RPC |
| `state/` | 关系图、语义身份、anchor、规范边和分支缓存 | 渲染样式、进程管理 |
| `fetch/` | LSP/Tree-sitter 查询、协议适配和数据归一化 | 决定节点在画布上的坐标 |
| `tui/` | 事件与组件编排；search/save/help 分离弹窗，config editor 管理终端挂起，canvas 分离布局与连线 | 持有语言服务器子进程、定义缓存语义 |
| `ipc/` | Unix socket 生命周期和外部消息协议 | 直接修改终端组件 |
| `export/` | 把已知可达关系稳定序列化，并安全创建新文件 | 读取终端输入、覆盖已有目标 |

## 当前数据流

```text
CLI ──> main ──> project config ──> App
               │
terminal event ├──> App state transition ──> render
               ├──> ec ──> restore terminal ──> $EDITER ──> reload config ──> graph refresh
               ├──> double-click ──> IPC event sender ──> editor clients
editor clients ──> IPC readers ──> bounded command channel ──> App
                                                         └──> IPC responder
               │
               ├──> WorkspaceSymbolClient ─┬─> LSP actor ──> language server
               └──> HierarchyClient ───────┴─> Tree-sitter shared index
                               │
                               └── result channels ──> App
language server ──> progress/status notification ──> AnalysisStatus ──> render
```

`main` 拥有 `LspProvider` 或 `TreeSitterProvider`。TUI 只收到 Fetch 顶层可克隆的 `WorkspaceSymbolClient` 与 `HierarchyClient`，既不能关闭语言服务器，也不认识 Tree-sitter parser/index。这一拆分可以防止短生命周期查询任务意外终止 LSP 会话，并保证两种 Tree-sitter 查询复用一次索引。

IPC server 同样不持有 App。每连接 reader 完成 framing 和协议验证后，只把 typed command 与 responder 放入有界 channel；TUI 在事件循环线程调用 App，再把结构化结果送回原连接。这样 socket I/O、业务状态迁移和 Ratatui 渲染保持单向依赖。

LSP actor 还拥有独立的状态通知通道。TUI 的 `status` 模块将 LSP 专用更新转换为 App 的 `AnalysisStatus`；没有可用 LSP 时，`main` 初始化 Tree-sitter grammar/query 并写入同一状态模型。全局分析状态和单次 workspace symbol 搜索状态是两个不同状态机；Tree-sitter 首次索引由查询 task 承担，搜索 modal/分支 loading 状态负责表示该次工作。终端 raw mode、备用屏幕、鼠标捕获和 OSC 52 副作用集中在 `tui/terminal.rs`，其他组件不直接操作 Crossterm 生命周期。

当前每个 cgraph 进程独立启动语言服务器。rust-analyzer 的内存索引无法跨进程直接复用，以及未来 workspace daemon 的候选设计，详见 [rust-analyzer 生命周期与索引复用设计](fetch/rust-analyzer.md)。

## 依赖方向

期望的长期依赖方向为：

```text
main -> cli / app / fetch / tui / ipc
tui  -> app / state / fetch 的窄接口 / export
app  -> state
fetch -> state
ipc  -> state
export -> state
```

`WorkspaceSymbolMatch` 位于 Fetch 公共层并由两种 provider 返回。TUI 仍使用 `tower_lsp::SymbolKind` 做 call/type 分类，这是当前唯一的协议枚举泄漏；增加无法自然映射的新 provider 时，应替换为项目级类型。

## 生命周期

1. 解析 CLI，从 workspace 根目录读取并校验 `.cgraph.toml`。
2. 如果给出 `--ipc-socket`，在进入终端前安全 bind 并创建 ownership marker；失败直接返回普通启动错误。
3. 确定显式或自动检测的语言服务器。
4. 尝试启动 LSP；失败或显式禁用时初始化支持语言的 Tree-sitter provider。两者都不可用才让搜索/展开进入可见错误。
5. 初始化终端 raw mode、备用屏幕和鼠标捕获。
6. 运行事件循环；`ec` 临时恢复终端并等待编辑器，返回后重新进入 TUI、重载配置并刷新已加载分支。
7. 无论事件循环是否成功，都尝试恢复终端。
8. 停止 IPC actor 并按 inode 安全清理本实例 socket；再按 LSP 规范发送 `shutdown` 和 `exit`，超时后回收子进程。

终端恢复和子进程回收属于必须保持的安全属性。修改 `main.rs` 或 `tui::init/restore` 时，需要验证错误路径，而不只验证正常退出。

## 状态与副作用原则

- App 方法应尽量是可测试的同步状态迁移。
- 外部 I/O 结果携带 request id；App 负责拒绝来自已关闭搜索会话、已删除分支或较早 hierarchy 重试的过期结果。IPC 请求也使用 request id 关联响应，但每条命令本身是一次原子 App 状态迁移。
- 查询失败必须显式表示为错误，不能用空结果伪装成功。
- `NodeId` 是进程内节点句柄，resolved 语义身份用于全局节点去重；两者不能混用，缺少位置的 provisional 节点需要在 prepare 后显式解析或合并。
- 收起分支只影响展示状态，不应清除已经获取的数据。
- 刷新只替换一层结果，并保留仍然存在的子节点实例及其展开状态。

## 后续结构性工作

- 为大图增加图版本号和布局快照缓存，避免无状态变化时每帧重算 SCC 与 rank。
- 为 Tree-sitter 索引增加文件变更失效、取消、规模上限和可观测进度；当前索引在会话内构建一次。
- 为 LSP 查询增加超时、日志轮转和服务端崩溃后的恢复策略；请求取消、基础 progress、空查询统计与每会话 stderr 文件已经实现。
- 为 IPC 增加 capability handshake、实例发现和真实 Neovim/PTY 端到端测试；双向 NDJSON、入站限制、App command 路由、实例路径和 socket 清理规则已经固化。

各模块的具体难点与 TODO 记录在对应目录的 README 中。
