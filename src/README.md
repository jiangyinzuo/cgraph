# ctree 源代码设计

本文档面向维护者，描述 `src/` 内部边界。产品愿景位于仓库根目录 `DESIGN.md`，用户可见行为以[需求树](../requirements/README.md)中对应需求为准；当实现和产品需求不一致时，应先记录差异，再决定修改代码还是需求文档。

## 需求追踪

内部设计不重复定义用户行为。主要模块与父需求的关系如下：

| 模块 | 主要需求 |
| --- | --- |
| `main.rs` / `cli.rs` | [REQ-1 会话与启动](../requirements/REQ-1-session/README.md)、[REQ-8 语言支持](../requirements/REQ-8-language-support/README.md) |
| `config/` | [REQ-9 项目本地配置与符号过滤](../requirements/REQ-9-project-configuration/README.md) |
| `app.rs` / `state/` | [REQ-3 层次关系探索](../requirements/REQ-3-hierarchy/README.md)、[REQ-4 画布与导航](../requirements/REQ-4-canvas-navigation/README.md)、[REQ-5 符号与树管理](../requirements/REQ-5-symbol-management/README.md) |
| `fetch/` | [REQ-2 分析后端状态](../requirements/REQ-2-analysis-status/README.md)、[REQ-3 层次关系探索](../requirements/REQ-3-hierarchy/README.md)、[REQ-5 符号与树管理](../requirements/REQ-5-symbol-management/README.md) |
| `tui/` | [REQ-2 分析后端状态](../requirements/REQ-2-analysis-status/README.md)、[REQ-4 画布与导航](../requirements/REQ-4-canvas-navigation/README.md)、[REQ-5 符号与树管理](../requirements/REQ-5-symbol-management/README.md) |
| `ipc/` | [REQ-6 进程间通信](../requirements/REQ-6-ipc/README.md) |

测试分层、逐文件清单、回归用例规则和尚未覆盖的测试类型统一维护在 [测试设计与覆盖总账](testing/README.md)。任何测试增删、移动或类型变化都必须同步刷新该文档；`tests/test_documentation.rs` 会自动检查逐文件数量，防止清单静默过期。

## 模块职责

| 模块 | 当前职责 | 不应承担的职责 |
| --- | --- | --- |
| `main.rs` | 组装 CLI、语言服务器、App 和终端生命周期 | 保存交互状态、解析 LSP 消息 |
| `cli.rs` | 命令行语法和启动配置 | 自动修改 App、启动外部进程 |
| `config/` | 读取并校验 workspace 根目录的 `.ctree.toml` | 查询 LSP、修改树或渲染错误弹窗 |
| `app.rs` / `app/search.rs` | UI 无关的交互状态迁移；搜索子模块负责模糊评分与稳定排序 | 直接读终端、直接发送 JSON-RPC |
| `state/` | 节点、语义身份、分支缓存和画布领域模型 | 渲染样式、进程管理 |
| `fetch/` | LSP/Tree-sitter 查询、协议适配和数据归一化 | 决定节点在画布上的坐标 |
| `tui/mod.rs` / `tui/canvas.rs` | 事件与组件编排；canvas 子模块负责纯世界布局、投影、碰撞和连线 | 持有语言服务器子进程、定义缓存语义 |
| `ipc/` | Unix socket 生命周期和外部消息协议 | 直接修改终端组件 |

## 当前数据流

```text
CLI ──> main ──> project config ──> App
               │
terminal event ├──> App state transition ──> render
               │
               ├──> WorkspaceSymbolClient ──> LSP actor ──> language server
               └──> HierarchyClient ───────────┘
                               │
                               └── result channels ──> App
language server ──> progress/status notification ──> AnalysisStatus ──> render
```

`main` 拥有 `LspProvider`，因此也拥有语言服务器进程的生命周期。TUI 只收到可克隆的 `WorkspaceSymbolClient` 与 `HierarchyClient`，不能关闭或替换进程。这一拆分可以防止短生命周期查询任务意外终止整个 LSP 会话。

LSP actor 还拥有独立的状态通知通道。TUI 将 LSP 专用更新转换为 App 的 `AnalysisStatus`；没有可用 LSP 时，`main` 可以初始化 Tree-sitter grammar/query 并写入同一状态模型。全局分析状态和单次 workspace symbol 搜索状态是两个不同状态机，Tree-sitter parser ready 不会让 LSP-only 搜索显示为可用。

当前每个 ctree 进程独立启动语言服务器。rust-analyzer 的内存索引无法跨进程直接复用，以及未来 workspace daemon 的候选设计，详见 [rust-analyzer 生命周期与索引复用设计](fetch/rust-analyzer.md)。

## 依赖方向

期望的长期依赖方向为：

```text
main -> cli / app / fetch / tui / ipc
tui  -> app / state / fetch 的窄接口
app  -> state
fetch -> state
ipc  -> state
```

目前 TUI 仍直接认识 LSP 的 workspace symbol 类型，并在渲染层完成一部分归一化。这是早期实现的已知边界泄漏。后续应把 `SymbolCandidate` 移到领域层或查询协调层，让 TUI 不依赖 `tower_lsp` 类型。

## 生命周期

1. 解析 CLI，从 workspace 根目录读取并校验 `.ctree.toml`。
2. 确定显式或自动检测的语言服务器。
3. 尝试启动 LSP。失败不会阻止 TUI 启动，而会成为搜索弹窗中的可见错误。
4. 初始化终端 raw mode、备用屏幕和鼠标捕获。
5. 运行事件循环，直到 App 进入退出状态或发生错误。
6. 无论事件循环是否成功，都尝试恢复终端。
7. 按 LSP 规范发送 `shutdown` 和 `exit`，超时后回收子进程。

终端恢复和子进程回收属于必须保持的安全属性。修改 `main.rs` 或 `tui::init/restore` 时，需要验证错误路径，而不只验证正常退出。

## 状态与副作用原则

- App 方法应尽量是可测试的同步状态迁移。
- 外部 I/O 结果携带 request id；App 负责拒绝来自已关闭搜索会话、已删除分支或较早 hierarchy 重试的过期结果。
- 查询失败必须显式表示为错误，不能用空结果伪装成功。
- `NodeId` 是进程内节点句柄，resolved 语义身份用于全局节点去重；两者不能混用，缺少位置的 provisional 节点需要在 prepare 后显式解析或合并。
- 收起分支只影响展示状态，不应清除已经获取的数据。
- 刷新只替换一层结果，并保留仍然存在的子节点实例及其展开状态。

## 近期结构性工作

- 按[有向关系图领域模型决策](state/graph-model.md)把递归树迁移为 `StableDiGraph`、anchor 可见图和 SCC 分层布局。
- 将 `App` 拆为 canvas、modal、command-prefix 等子状态，避免单一结构持续膨胀。
- 为 Tree-sitter 实现 `HierarchyQuery` / `HierarchyResponse`，复用当前 LSP 领域边界。
- 为 LSP 查询增加超时、日志和服务端崩溃后的恢复策略；请求取消和基础 progress 展示已经实现。
- 固化 IPC 帧格式、版本协商、单实例和 socket 清理规则。

各模块的具体难点与 TODO 记录在对应目录的 README 中。
