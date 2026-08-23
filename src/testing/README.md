# 测试设计与覆盖总账

本文档是 ctree 自动化测试的内部设计总账，面向维护者。产品需求位于 [`requirements/`](../../requirements/README.md)，用户操作位于 [`docs/`](../../docs/README.md)；本文件只说明测试分层、覆盖策略、维护规则和当前缺口。

## 维护契约

新增、删除、移动或重命名测试时，必须在同一个变更中更新本文档：

1. 更新“自动化清单”中的对应文件数量和总数。
2. 如果测试引入新的测试类型，更新“测试分层”。
3. 如果测试覆盖了“当前缺少的测试”中的项目，删除或缩小对应缺口。
4. 如果实现了新的用户可观察行为，记录它采用的核心断言；回归缺陷优先写能复现原问题的针对性测试。
5. 运行 `cargo test --all-targets`。`tests/test_documentation.rs` 会扫描 Rust 源文件中的 `#[test]` 与 `#[tokio::test]`，并与下方机器可读清单逐文件比较；忘记刷新数量时测试会失败。

数量校验只能证明清单已刷新，不能判断文字说明是否准确。评审时仍需检查新增测试是否改变了分层、覆盖边界或未来计划。

## 自动化清单

当前共有 **58 个自动化测试**，其中 57 个验证产品代码，1 个验证本测试总账与源码注解保持一致。

<!-- test-inventory
src/app.rs: 13
src/cli.rs: 3
src/config/mod.rs: 2
src/fetch/lsp.rs: 10
src/fetch/treesitter.rs: 2
src/ipc/protocol.rs: 1
src/main.rs: 1
src/state/graph.rs: 7
src/tui/mod.rs: 18
tests/test_documentation.rs: 1
total: 58
-->

| 位置 | 数量 | 类型 | 主要覆盖 |
| --- | ---: | --- | --- |
| `src/app.rs` | 13 | 状态机单元测试 | 搜索生命周期、项目符号过滤、模糊排序、根管理、hierarchy 加载/缓存/去重/竞态 |
| `src/cli.rs` | 3 | 解析测试 | call 子命令、空画布、LSP 参数位置 |
| `src/config/mod.rs` | 2 | 配置与匹配单元测试 | 缺省加载、严格 TOML、模式规范化、大小写和 `*` 通配符 |
| `src/fetch/lsp.rs` | 10 | 协议与异步集成测试 | JSON-RPC、workspace symbol、call/type hierarchy、取消、进度、安全限制 |
| `src/fetch/treesitter.rs` | 2 | 文件系统集成测试 | 语言检测、四种 grammar/query 初始化与解析 |
| `src/ipc/protocol.rs` | 1 | 序列化契约测试 | 版本化、带类型标签的 IPC 请求 |
| `src/main.rs` | 1 | 组装/降级测试 | Tree-sitter fallback 与统一状态 |
| `src/state/graph.rs` | 7 | 图领域模型单元测试 | 菱形全局去重、双向边观察、循环、自环、身份隔离与解析、共享边清除 |
| `src/tui/mod.rs` | 18 | 输入、布局和渲染组件测试 | 键鼠映射、拖拽平移、空间导航、图分层、菱形去重、循环/自环样式、碰撞、连线、footer 和终端缓冲区 |
| `tests/test_documentation.rs` | 1 | 仓库一致性测试 | 本清单与测试注解逐文件一致 |

`examples/` 会被 `cargo test --all-targets` 编译，但当前没有测试函数，因此不计入上述数量。

## 测试分层

### 领域模型单元测试

直接构造树迁移期间的 `Node`/`Branch`、新 `RelationGraph` 和语义身份，不启动 Tokio、终端或外部进程。断言重点是状态不变量，例如左右分支互不影响、全局语义节点去重、规范边观察、循环安全遍历、provisional identity 解析以及共享边不会被单个分支误删。

这类测试应保持小、快、无 I/O。若一个行为可以在 State 层证明，不应只依赖更昂贵的 TUI 测试间接覆盖。

### App 状态机测试

App 测试把异步 I/O 表示为显式请求和显式完成事件：先调用状态迁移取得 request，再注入成功或失败结果。这样可以确定性地覆盖真实 UI 最容易出错的竞态，而不需要 sleep。

Hierarchy 测试重点验证：

- 首次展开只生成目标方向的一个请求。
- 成功结果进入分支缓存，收起再展开不重复请求。
- 加载期间允许收起，完成时尊重用户最新可见性。
- 失败可以重试，新 request id 使旧响应失效。
- 成功空结果进入 `Loaded`，不能与 `Failed` 混淆。
- incoming 和 outgoing 各自在分支内去重，但同一语义的跨方向关系都要保留。
- 项目过滤在写入搜索候选与 hierarchy 缓存前按完整限定名执行。

搜索测试使用同样模式验证防抖前状态、请求开始、旧会话结果拒绝和本地模糊排序。

### LSP 协议测试

LSP 测试使用 `tokio::io::duplex` 连接真实 `JsonRpcClient` actor 与测试中的模拟 server。测试读写标准 `Content-Length` 帧，而不是直接 mock `HierarchyClient::query` 的返回值，因此实际覆盖：

- JSON 编解码与 request id 路由。
- server 通知和反向请求与普通响应并发出现。
- `prepareCallHierarchy` 后继续 incoming/outgoing 请求。
- `prepareTypeHierarchy` 后继续 supertypes/subtypes 请求。
- prepare item 中的协议数据传入第二阶段请求。
- future 被 abort 后发送 `$/cancelRequest`。
- 非法超大消息在分配前被拒绝。

模拟 server 让协议测试稳定、快速且不依赖本机工具链。真实 rust-analyzer、clangd 和 pylsp 目前属于手工诊断边界，不作为普通单元测试的成功条件。

### TUI 输入测试

输入测试构造 Crossterm `KeyEvent` / `MouseEvent`，调用生产代码使用的事件映射函数。它们验证完整命令语义，例如裸 `t` 只进入前缀状态、`tl` 不会把 `l` 解释为空间导航、点击侧按钮先选择节点再只操作对应分支。

键盘和鼠标应有对称覆盖：核心行为若同时暴露给两种输入方式，至少各有一个测试经过对应入口，不能只测试 App 方法。

### 纯布局测试

世界布局器以树和选择为输入，屏幕投影再加入终端区域与 viewport；`canvas_edges` 根据可见投影生成连线单元。测试直接检查几何不变量：

- 收起分支的孩子不进入可见布局。
- 选择所在树位于世界原点，多个根具有不同且稳定的世界槽位。
- 所有展开节点都进入世界布局，包含按钮的完整世界矩形两两不相交，包括限定名产生的不同宽度节点。
- 视口投影只返回完整可见节点，屏外节点平移后能够进入可见集合。
- 每个可见父子关系产生带正确 `parent_id` / `child_id` 的边。

碰撞测试必须比较完整矩形，不能退化为“左上角坐标不同”；后者无法发现半行偏移造成的真实覆盖。

拖拽回归测试使用不足以同时显示父子节点的窄画布，分别从节点主体和空白背景发起左键拖拽。它断言 viewport 按相邻鼠标事件的增量变化、原本屏外的孩子进入投影，同时世界布局、`NodeId` 和树结构保持不变；另有按钮测试确认 toggle 不会留下拖拽锚点。

### 终端渲染测试

Ratatui `TestBackend` 提供虚拟终端 Buffer。测试调用完整 `render()`，再读取最终单元格字符，覆盖“布局数据正确但渲染忘记使用”的缺陷。

当前关键断言包括：

- 父子方框之间的终端单元确实出现连接字符。
- 节点渲染发生在连线之后，不会清除框间连接。
- 节点顶部边框不包含 `call` / `type` 角标。
- 终端容得下节点时，动态宽度方框不会截断完整的 `Class::method`。
- 快捷键提示与分析状态出现在同一条最底行，状态不会残留在画布区域。

目前不使用整屏 golden snapshot；关键字符和结构断言对颜色、空白和非功能样式调整更稳定。

### 文件系统与组装测试

Tree-sitter 测试创建临时工作区，实际初始化 Rust、C、C++、Python grammar/query 并解析最小源码。启动层测试通过临时 Python 工作区验证没有 LSP 时的 fallback 和 `AnalysisStatus` 映射。

临时目录必须使用唯一名称，测试结束后删除；测试内容不得依赖开发者真实工作区。

### CLI 与协议契约测试

CLI 测试使用 Clap `try_parse_from`，IPC 测试检查稳定 JSON 形状。此类测试保护用户脚本和外部客户端依赖的接口，内部重构不能无意改变它们。

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

### 真实语言服务器自动化测试

CI 当前不会真正启动 rust-analyzer、clangd 或 pylsp。冷索引、工具版本、后台文件变化和 `content modified` 会带来时间与稳定性问题。现阶段使用内存协议 server，并提供 `examples/lsp_workspace_symbols.rs` 和 `examples/lsp_hierarchy.rs` 手工诊断。

后续可以增加带固定最小 fixture、固定 server 版本、显式超时和 capability 检查的可选测试组；它不应默认拖慢普通 `cargo test`。

### 完整 PTY 端到端测试

目前没有启动真实 `ctree` 二进制、通过伪终端发送键盘/鼠标序列、检查备用屏幕并验证退出后 terminal mode 恢复。未来可以使用 PTY fixture 覆盖启动、搜索、展开、退出和异常恢复，但需要隔离不同终端实现。

### Snapshot / golden 测试

没有保存完整终端截图。若以后 UI 稳定，可以为少量关键屏幕增加经审核的 snapshot；不能用大量脆弱快照替代几何和状态断言。

### 属性测试与模糊测试

目前没有用 `proptest` 随机生成任意深度树、终端尺寸、节点数量、Unicode 名称或乱序异步结果。优先候选包括：布局永不重叠、所有 placement 位于 bounds、连线端点对应可见节点、任意旧 request id 都不能覆盖新状态。

### 性能与压力测试

没有测量数千节点布局、超深树递归、大量并发 hierarchy 请求、超大 workspace symbol 响应或长时间事件循环的内存增长。未来应将基准和正确性压力测试分开，避免普通测试套件受机器性能影响。

### 跨平台与终端兼容测试

没有自动覆盖 Windows Terminal、不同 `$TERM`、Unicode 宽度差异、终端 resize 风暴和鼠标协议差异。当前 `TestBackend` 只验证逻辑缓冲区，不等同于真实终端兼容性。

### Tree-sitter hierarchy 测试

Tree-sitter 当前只测试 grammar/query 初始化，没有 workspace symbol 或 hierarchy 语义。实现 provider 后，需要按语言分别测试支持范围、不确定结果和“不支持”状态，不能把未知关系伪装成成功空数组。

### 配置热重载与模式属性测试

当前覆盖启动时缺省/有效/无效配置、重复模式、大小写边界和多处 `*` 的确定性匹配，但没有运行时热重载，也没有用属性测试将通配算法与参考 glob 实现比较。若未来扩展 `?`、字符组或 regex，需要增加拒绝非法模式、最坏输入耗时和 Unicode 边界用例，不能只验证文档中的三个示例。

### 刷新、重复节点、IPC 与导出

这些需求尚未完整实现，因此也没有对应的端到端行为测试。实现时至少需要覆盖：

- 刷新只替换一层，并保留仍存在孩子的 `NodeId` 与深层展开状态。
- 相同语义节点可以有多个实例，选择时同步强调但不错误去重路径。
- IPC 断帧、版本不兼容、socket 清理和多个客户端。
- 导出不覆盖已有文件，失败不留下半成品。

## 验证命令

开发时从最相关测试开始，再运行完整检查：

```bash
cargo test hierarchy
cargo test expanded_node_rectangles_never_overlap
cargo test visible_parent_child_relationship_renders_a_connector

cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo doc --no-deps
```

`cargo test --all-targets` 同时编译 examples，并执行测试总账一致性检查。Markdown 相对链接检查和 `git diff --check` 仍属于交付前仓库检查，不由 Rust 测试替代。
