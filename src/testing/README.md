# 测试设计与覆盖总账

本文档是 cgraph 自动化测试的内部设计总账，面向维护者。产品需求位于 [`requirements/`](../../requirements/README.md)，用户操作位于 [`docs/`](../../docs/README.md)；本文件只说明测试分层、覆盖策略、维护规则和当前缺口。

## 维护契约

新增、删除、移动或重命名测试时，必须在同一个变更中更新本文档：

1. 更新“自动化清单”中的对应文件数量和总数。
2. 如果测试引入新的测试类型，更新“测试分层”。
3. 如果测试覆盖了“当前缺少的测试”中的项目，删除或缩小对应缺口。
4. 如果实现了新的用户可观察行为，记录它采用的核心断言；回归缺陷优先写能复现原问题的针对性测试。
5. 运行 `cargo test --all-targets`。`tests/test_documentation.rs` 会扫描 Rust 源文件中的 `#[test]` 与 `#[tokio::test]`，并与下方机器可读清单逐文件比较；忘记刷新数量时测试会失败。

数量校验只能证明清单已刷新，不能判断文字说明是否准确。评审时仍需检查新增测试是否改变了分层、覆盖边界或未来计划。

## 自动化清单

当前共有 **118 个自动化测试**，其中 117 个验证产品代码，1 个验证本测试总账与源码注解保持一致。

<!-- test-inventory
src/app.rs: 17
src/app/config.rs: 1
src/app/save.rs: 1
src/cli.rs: 4
src/config/mod.rs: 2
src/export/mod.rs: 3
src/fetch/lsp.rs: 15
src/fetch/lsp/symbol_names.rs: 5
src/fetch/treesitter.rs: 9
src/ipc/tests.rs: 5
src/ipc/protocol.rs: 2
src/main.rs: 2
src/state/graph.rs: 9
src/tui/canvas/connections.rs: 4
src/tui/config_editor.rs: 2
src/tui/help.rs: 1
src/tui/messages.rs: 2
src/tui/mod.rs: 30
src/tui/save.rs: 2
src/tui/search.rs: 1
tests/test_documentation.rs: 1
total: 118
-->

| 位置 | 数量 | 类型 | 主要覆盖 |
| --- | ---: | --- | --- |
| `src/app.rs` | 17 | 状态机单元测试 | 搜索生命周期、项目符号过滤、模糊排序、根管理、hierarchy 加载/缓存/刷新/去重/竞态、Tree-sitter 提示和外部聚焦 |
| `src/app/config.rs` | 1 | 配置重载状态机测试 | 全部成功空/已加载/正在刷新分支、新 request id、过滤应用、anchor 保留与迟到结果拒绝 |
| `src/app/save.rs` | 1 | 保存状态单元测试 | 路径编辑、错误保留与再次编辑恢复 |
| `src/cli.rs` | 4 | 解析测试 | call 子命令、空画布、LSP 参数位置、IPC socket 路径 |
| `src/config/mod.rs` | 2 | 配置与匹配单元测试 | 缺省加载、严格 TOML、模式规范化、大小写和 `*` 通配符 |
| `src/export/mod.rs` | 3 | 序列化与文件系统测试 | 稳定人类可读格式、共享节点/循环、空路径、安全创建与禁止覆盖 |
| `src/fetch/lsp.rs` | 15 | 协议与异步集成测试 | JSON-RPC、workspace/document symbol、call/type hierarchy、动态 capability、未注册 type hierarchy 的 Tree-sitter 回退、Pyrefly 命令与 Python 根解析、取消、进度、安全限制、UTF-16 position 协商，以及条件执行的真实 Pyrefly/rust-analyzer 最小工作区 |
| `src/fetch/lsp/symbol_names.rs` | 5 | LSP 方言适配单元测试 | server 识别、Rust inherent/trait impl、Pyrefly 点号限定名、非法 detail 降级、通用协议边界 |
| `src/fetch/treesitter.rs` | 9 | 文件系统与静态索引集成测试 | 语言检测、四种 grammar、Rust/Python/C/C++ 符号与调用、Rust/C++/Python 类型关系、限定方法名、UTF-16 列归一化、目录排除、歧义拒绝和取消安全的单次索引 |
| `src/ipc/tests.rs` | 5 | Unix socket 异步集成测试 | 私有父目录、`0600` 权限、多客户端广播、请求路由与相同 request id 响应、版本拒绝、1 MiB 入站限制、陈旧 socket 回收和 inode 保护 |
| `src/ipc/protocol.rs` | 2 | 序列化契约测试 | 版本化 tagged request、无 request id 的 UTF-16 零基打开位置事件 |
| `src/main.rs` | 2 | 组装/降级测试 | Python 默认 Pyrefly、显式 pylsp 覆盖、Tree-sitter fallback 与统一状态 |
| `src/state/graph.rs` | 9 | 图领域模型单元测试 | 菱形全局去重、双向边观察、循环、自环、身份解析、边迁移、共享边清除、可见/已知图投影 |
| `src/tui/canvas/connections.rs` | 4 | 连线几何与渲染单元测试 | 正交圆角、真实普通交叉高亮、单/双线 `╪` / `╫` 轴向语义、极远线段先裁剪后栅格化 |
| `src/tui/config_editor.rs` | 2 | 外部进程与选择单元测试 | `$EDITER` 优先、`$EDITOR` 回退、缺失诊断、真实子进程收到准确配置路径和最小模板 |
| `src/tui/help.rs` | 1 | 帮助输入与渲染集成测试 | Shift-`?`、完整内容、Canvas 拦截、键鼠滚动、小终端裁剪、关闭和状态稳定 |
| `src/tui/messages.rs` | 2 | 消息 pager 组件测试 | less 风格导航、Unicode 换行、`V` 行选择、软换行原文复制、`q` 关闭、保留底行和最多 15 行布局 |
| `src/tui/mod.rs` | 30 | 输入、布局和渲染组件测试 | 键鼠映射、`ec` 前缀、固定 footer、倒数第二行消息、`g<` pager 保留底栏、无边框画布标题与选中 URI、保存、IPC、位置锚定、刷新、拖拽/裁剪、空间导航、布局、连线和终端缓冲区 |
| `src/tui/save.rs` | 2 | 保存弹窗组件测试 | 已有目标保持不变并显示错误、成功写入并显示路径 |
| `src/tui/search.rs` | 1 | 搜索展示单元测试 | Rust/C++ `::` 与 Python `.` 限定名不重复追加 container 标签 |
| `tests/test_documentation.rs` | 1 | 仓库一致性测试 | 本清单与测试注解逐文件一致 |

`examples/` 会被 `cargo test --all-targets` 编译，但当前没有测试函数，因此不计入上述数量。

## 测试分层

### 领域模型单元测试

直接构造 `RelationGraph`、`GraphNode`、`GraphBranch` 和语义身份，不启动 Tokio、终端或外部进程。断言重点是状态不变量，例如左右分支互不影响、全局语义节点去重、规范边 observation、循环安全遍历、provisional identity 解析以及共享边不会被单个分支误删。

这类测试应保持小、快、无 I/O。若一个行为可以在 State 层证明，不应只依赖更昂贵的 TUI 测试间接覆盖。

### App 状态机测试

App 测试把异步 I/O 表示为显式请求和显式完成事件：先调用状态迁移取得 request，再注入成功或失败结果。这样可以确定性地覆盖真实 UI 最容易出错的竞态，而不需要 sleep。

Hierarchy 测试重点验证：

- 首次展开只生成目标方向的一个请求。
- 成功结果进入分支缓存，收起再展开不重复请求。
- 加载期间允许收起，完成时尊重用户最新可见性。
- 失败可以重试，新 request id 使旧响应失效。
- 成功空结果进入 `Loaded`，不能与 `Failed` 混淆。
- incoming 和 outgoing 结果复用全局语义节点，同一语义的跨方向关系和 edge observation 都保留。
- 刷新同时生成两个方向的新 request id，成功只替换一层并保留复用节点的深层状态，失败不清空旧缓存。
- 项目过滤在写入搜索候选与 hierarchy 缓存前按完整限定名执行。
- Tree-sitter 成功结果写入明确的语法级置信度提示，不把静态子集冒充完整语义。
- 配置重载刷新每个已加载或正在刷新的可达方向，包括成功空分支；新 request id 拒绝编辑期间的旧结果，新过滤器作用于响应且 anchors 保留。

搜索测试使用同样模式验证防抖前状态、请求开始、旧会话结果拒绝和本地模糊排序。

### LSP 协议测试

LSP 测试使用 `tokio::io::duplex` 连接真实 `JsonRpcClient` actor 与测试中的模拟 server。测试读写标准 `Content-Length` 帧，而不是直接 mock `HierarchyClient::query` 的返回值，因此实际覆盖：

- JSON 编解码与 request id 路由。
- server 通知和反向请求与普通响应并发出现。
- `prepareCallHierarchy` 后继续 incoming/outgoing 请求。
- `prepareTypeHierarchy` 后继续 supertypes/subtypes 请求。
- type hierarchy 的动态注册/注销会改变可用能力；未注册时混合 client 不向 LSP 发送请求，并由 Rust Tree-sitter trait impl fixture 返回 parent。
- prepare item 中的协议数据传入第二阶段请求。
- rust-analyzer 风格的 `Function` call item 与签名 detail 会触发 document symbol 请求，并用 `impl Type` container 生成 `Type::method`；同 URI 的重复 item 只请求一次。
- future 被 abort 后发送 `$/cancelRequest`。
- 非法超大消息在分配前被拒绝。

模拟 server 仍是稳定、快速且不依赖本机工具链的主要协议测试。另有两个条件执行的真实 server 测试：`pyrefly` 位于 `PATH` 时验证 `pyrefly lsp`、受控 `didOpen` 索引引导、workspace symbol、incoming call hierarchy 和 `Worker.run` 名称；`rust-analyzer` 位于 `PATH` 时验证最小 Cargo workspace、workspace symbol 和 outgoing call hierarchy，并在索引尚未稳定而返回 `content modified` 时于总超时内重试。对应命令不存在或 `--version` 失败时，测试打印跳过原因并返回，不让缺少可选开发工具的普通 CI 失败。

### Tree-sitter 静态索引测试

Tree-sitter 测试创建真实临时工作区，经生产代码递归发现文件、编译 grammar/query、解析源码并惰性建立共享索引，不直接构造内部 `ProjectIndex`。四语言 fixture 覆盖：

- Rust `Type::method`、trait method、直接函数/方法调用、incoming/outgoing 反向映射和 `target` 排除。
- Python `Class.method`、函数调用、supertype/subtype 双向关系，以及多个类同名方法时 `self.method()` 优先绑定当前类。
- C 函数调用以及 prototype 不被误当成可查询定义。
- C++ 类外 `Class::method` 定义、函数调用和 base-class 关系。
- 跨文件同名目标不任意绑定；无精确位置的同名根返回 ambiguous 错误。
- 首个搜索等待任务在构建中被取消后，后台索引继续完成，下一请求复用同一次扫描。
- 非 ASCII 行前缀的 Tree-sitter UTF-8 byte column 会归一化为公共 UTF-16 character。

启动层测试再经过 `start_tree_sitter_fallback` 获取真实 provider client，证明没有 LSP 时不仅状态为 Ready，workspace symbol 与 hierarchy 也实际可查询。动态分派等静态未知关系无法通过 fixture 证明“完整”，因此另由 App 测试断言用户始终看到语法级置信度提示。

### TUI 输入测试

输入测试构造 Crossterm `KeyEvent` / `MouseEvent`，调用生产代码使用的事件映射函数。它们验证完整命令语义，例如裸 `t` 只进入前缀状态、`tl` 不会把 `l` 解释为空间导航、点击侧按钮先选择节点再只操作对应分支。

键盘和鼠标应有对称覆盖：核心行为若同时暴露给两种输入方式，至少各有一个测试经过对应入口，不能只测试 App 方法。

选择稳定性测试记录目标节点交互前后的完整投影槽位：鼠标单击、鼠标 toggle 和键盘空间导航都必须保持相同位置。纯布局测试还比较不同 selection 下任意节点对的坐标差，保证选择只产生统一平移而不会交换同列顺序。异步完成测试故意把短临时名解析成长限定名，确认节点宽度改变时 viewport 仍保持其屏幕中心。

双击测试经过生产用 down/up 状态机，验证同一精确节点的第二次完整点击生成 `OpenLocation`，没有位置的 provisional 节点不生成事件，并验证未启用 sender 时错误进入消息摘要和历史。已有拖拽测试覆盖 drag 路径；double-click 状态会在 drag 时清除，因此不会把拖动结束误判为打开位置。

`ec` 测试确认裸 `e` 和错误后缀没有副作用，只有完整前缀产生编辑请求。帮助测试从带 Shift modifier 的真实 `?` 入口进入，读取最终 TestBackend 字符，确认完整清单包含配置、搜索、保存和帮助操作；帮助打开后 Canvas 命令被拦截，鼠标滚轮更新 scroll，关闭不修改图、viewport 或退出状态。

### 纯布局测试

世界布局器以 RelationGraph 和选择为输入，从 anchors/expanded branches 生成可见图，执行 SCC 分层，再把节点和边一起投影为 `CanvasLayoutSnapshot`。测试直接检查几何不变量：

- 收起分支的孩子不进入可见布局。
- 选择所在 anchor 位于世界原点，多个入口具有不同且稳定的世界槽位。
- 所有展开节点都进入世界布局，包含按钮的完整世界矩形两两不相交，包括限定名产生的不同宽度节点。
- 视口投影保留与 viewport 相交的节点真实切片，完全离屏才排除；平移后局部节点能够完整进入可见集合。
- 菱形关系只生成一个共享 placement，同时保留四条关系边。
- 循环和自环布局终止，并分别产生双线/箭头和 `↺` 特殊单元。
- 每个可见关系产生带正确 `source_id` / `target_id` 的边。

碰撞测试必须比较完整矩形，不能退化为“左上角坐标不同”；后者无法发现半行偏移造成的真实覆盖。

拖拽回归测试使用不足以完整显示相邻节点的窄画布，分别从节点主体和空白背景发起左键拖拽。它断言 viewport 按相邻鼠标事件的增量变化、初始局部可见的节点完整进入 viewport，同时世界布局、`NodeId` 和图结构保持不变；另有按钮测试确认 toggle 不会留下拖拽锚点。裁剪回归测试把节点左侧移出 viewport，检查边缘单元仍是真实的顶部横线而不是重新生成的左上角，并确认节点完全无交集后才从布局消失。

### 终端渲染测试

Ratatui `TestBackend` 提供虚拟终端 Buffer。测试调用完整 `render()`，再读取最终单元格字符，覆盖“布局数据正确但渲染忘记使用”的缺陷。

当前关键断言包括：

- 父子方框之间的终端单元确实出现连接字符。
- 前向边在目标节点前显示 `▶`，方向不依赖颜色。
- 同一边的正交转弯使用圆角且不高亮；不同边普通交叉为高亮 `┼`。
- 单线与双线交叉分别使用 `╪` / `╫`，字符保留特殊边所在轴向。
- 节点渲染发生在连线之后，不会清除框间连接。
- 部分离屏节点按完整 widget 的真实切片渲染，不在 viewport 边界制造假边框。
- 端点方框完全离屏后，只要路径仍穿过 viewport，屏内线段与边界箭头继续渲染。
- 节点顶部边框不包含 `call` / `type` 角标。
- 终端容得下节点时，动态宽度方框不会截断完整的 `Class::method`。
- 画布不再绘制最外层边框；左上角默认显示 `CALL GRAPH`，选中带精确位置的节点后显示其文件 URI。
- 快捷键提示与分析状态连续出现在同一条最底行，普通信息和错误只出现在倒数第二行；`g<` 展开后最底行仍可见。
- 默认 footer 只保留高频入口并提示 `?`，不再常驻显示 `w`、`dd/dp/dn` 等低频命令。
- 完整帮助 modal 在足够高的终端中显示所有分组和 `ec`，滚动状态不影响底层画布。

目前不使用整屏 golden snapshot；关键字符和结构断言对颜色、空白和非功能样式调整更稳定。

### 文件系统与组装测试

Tree-sitter 测试创建临时工作区，实际初始化 Rust、C、C++、Python grammar/query 并解析项目源码。启动层测试通过临时 Python 工作区验证没有 LSP 时的 fallback、`AnalysisStatus` 映射和可查询 client 组装。

临时目录必须使用唯一名称，测试结束后删除；测试内容不得依赖开发者真实工作区。

配置测试验证 `create_new` 最小模板不会覆盖已有内容。编辑器测试创建真实可执行脚本，经生产 `Command` 路径启动，并断言脚本收到 `<workspace>/.cgraph.toml`；环境变量选择单独验证项目约定的 `$EDITER` 优先和标准 `$EDITOR` 回退。终端 restore/resume 仍由后述 PTY 层负责，因为 TestBackend 不能表示操作系统 tty mode。

### IPC socket 集成测试

IPC 测试绑定真实临时 Unix socket，并使用两个真实 `tokio::net::UnixStream` 客户端逐行读取生产 actor 写出的数据，而不是直接调用 serde helper。核心断言包括：

- socket 权限为 `0600`，两个客户端收到完全相同的版本化 NDJSON `open_location`，行和 UTF-16 character 保持零基。
- 零客户端发送返回可诊断错误；每客户端独立 writer/有界队列不会把同步 TUI 绑定到 socket I/O。
- 普通文件不会被覆盖；只有 marker device/inode 匹配且 connect 被拒绝的 crash 遗留 socket 会回收。
- 旧实例退出时若路径已经被 replacement socket 占用，只清理旧 marker，不删除新 inode。
- 合法 `focus_symbol` 进入有界 command channel，responder 返回相同 request id 的 `accepted`；不兼容版本返回同 id 的结构化错误且不产生 command。
- 超过 1 MiB 的入站帧在 JSON 反序列化前拒绝，错误响应在关闭连接前排空。

App 测试另行验证精确位置与本地搜索使用同一 resolved identity、无位置唯一匹配、provisional 创建、同名歧义、空 file URI 和不完整坐标。TUI 测试把真实 `IpcCommand` 注入生产路由，验证成功修改 App 并返回 matching `accepted`，非法请求则返回 matching `error`。这些测试不启动 Neovim；真实 Neovim 进程联动仍属于后续可选端到端层。

### 导出测试

导出测试按副作用边界分成四层：State 测试证明 `known_graph` 包含收起缓存、排除清除后不可达节点；纯序列化测试构造共享节点和循环，重复渲染并比较完整字符串，同时检查局部编号、关系方向和从一开始的行列；文件系统测试使用唯一临时目录，验证新目标可创建且已有内容绝不被截断；TUI 测试分别经过 `w` 入口和保存弹窗的 Enter 处理，检查失败仍留在 modal、成功显示实际路径，并通过 `TestBackend` 验证错误确实可见。

这些层次刻意不依赖真实 LSP：图状态是导出模块的输入契约，语言服务器如何得到该状态应由 Fetch/App 测试负责。当前仍没有 PTY 级完整键入流程，也没有可靠模拟磁盘写满后部分文件清理的测试；这两项保留在后续缺口中。

### CLI 与协议契约测试

CLI 测试使用 Clap `try_parse_from`，IPC 协议测试检查 request 与 event 的稳定 JSON 形状，LSP capability 测试固定 UTF-16 协商。此类测试保护用户脚本和外部客户端依赖的接口，内部重构不能无意改变它们。

## 回归测试规则

修复用户可观察缺陷时，优先按以下顺序设计测试：

1. 用最小状态或输入复现缺陷，确认测试在旧实现上会失败。
2. 断言用户能观察到的结果，而不是只断言某个新辅助函数被调用。
3. 若缺陷跨层发生，保留一个最靠近根因的测试，再增加一个经过用户入口的窄集成测试。
4. 对竞态携带 request id，显式构造“旧结果晚于新结果到达”的顺序，不依赖线程调度碰巧复现。
5. 对布局遍历所有节点对或所有可见边，不只检查某个看起来合理的坐标。

例如“方框重叠”由纯布局测试两两比较完整矩形；“没有连线”同时检查 `CanvasEdge` 和 `TestBackend` 最终字符。这比截图人工观察更精确，也能定位是布局阶段还是渲染阶段出错。

## 当前缺少的测试

以下内容尚未自动覆盖。新增对应测试后，应在同一个变更中更新本节和自动化清单。

### 真实语言服务器覆盖缺口

当前已有条件执行的 Pyrefly 与 rust-analyzer 最小集成测试，并保留 `examples/lsp_workspace_symbols.rs` 和 `examples/lsp_hierarchy.rs` 供手工诊断。仍未自动覆盖 clangd、pylsp、固定 server 版本矩阵、冷/热索引性能、后台文件变化和 `content modified`。这些场景容易受工具版本与机器性能影响，后续应放入显式可选的测试组，不让普通正确性测试产生不稳定的时间门槛。

### 完整 PTY 端到端测试

目前没有启动真实 `cgraph` 二进制、通过伪终端发送键盘/鼠标序列、检查备用屏幕并验证退出后 terminal mode 恢复。未来可以使用 PTY fixture 覆盖启动、搜索、展开、`ec` 挂起/恢复、编辑器启动失败、退出和异常恢复，但需要隔离不同终端实现。

### Snapshot / golden 测试

没有保存完整终端截图。若以后 UI 稳定，可以为少量关键屏幕增加经审核的 snapshot；不能用大量脆弱快照替代几何和状态断言。

### 属性测试与模糊测试

目前没有用 `proptest` 随机生成任意深度树、终端尺寸、节点数量、Unicode 名称或乱序异步结果。优先候选包括：布局永不重叠、所有 placement 位于 bounds、连线端点对应可见节点、任意旧 request id 都不能覆盖新状态。

### 性能与压力测试

没有测量数千节点布局、超深树递归、大量并发 hierarchy 请求、超大 workspace symbol 响应或长时间事件循环的内存增长。未来应将基准和正确性压力测试分开，避免普通测试套件受机器性能影响。

### 跨平台与终端兼容测试

没有自动覆盖 Windows Terminal、不同 `$TERM`、Unicode 宽度差异、终端 resize 风暴和鼠标协议差异。当前 `TestBackend` 只验证逻辑缓冲区，不等同于真实终端兼容性。

### Tree-sitter 索引生命周期与规模

当前覆盖首次惰性构建、同一 provider 的查询复用，以及首个等待任务取消时构建继续的 single-flight 语义，但没有文件监听、索引失效、超大仓库规模上限、二进制/超大源码跳过策略或耗时进度通知。实现增量更新时应固定 fixture 修改顺序，验证新增、删除和重命名定义不会留下旧边；性能测试应与普通正确性套件分离。

### 配置模式属性测试

当前覆盖启动时缺省/有效/无效配置、模板安全创建、会话内重载、重复模式、大小写边界和多处 `*` 的确定性匹配，但没有用属性测试将通配算法与参考 glob 实现比较。若未来扩展 `?`、字符组或 regex，需要增加拒绝非法模式、最坏输入耗时和 Unicode 边界用例，不能只验证文档中的示例。

### IPC 与导出端到端

IPC 双向联动与导出已有领域、文件系统、协议和 TUI 组件测试，但还没有真实 PTY/Neovim 端到端及高负载覆盖。后续至少需要覆盖：

- 更全面的 malformed/partial frame 属性测试，以及大量客户端/请求下的队列公平性、响应关联和外部请求与本地输入顺序。
- 启动真实 Neovim、连接 socket、双击节点并验证 buffer/cursor 的完整联动。
- 导出在磁盘写满等中途写入失败时的文件清理策略，以及真实终端中的完整 `w` 输入流程。

## 验证命令

开发时从最相关测试开始，再运行完整检查：

```bash
cargo test hierarchy
cargo test expanded_node_rectangles_never_overlap
cargo test visible_parent_child_relationship_renders_a_connector
cargo test ipc

cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo doc --no-deps
```

`cargo test --all-targets` 同时编译 examples，并执行测试总账一致性检查。Markdown 相对链接检查和 `git diff --check` 仍属于交付前仓库检查，不由 Rust 测试替代。
