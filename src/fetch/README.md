# Fetch 层设计

Fetch 层负责从外部分析能力获得符号和层次关系，再归一化为 State 层类型。当前实现了 LSP workspace symbol、标准 call/type hierarchy，以及 Rust、C、C++、Python 的惰性 Tree-sitter 项目静态索引。

相关产品规范：[REQ-2 分析后端状态](../../requirements/REQ-2-analysis-status/README.md)、[REQ-3 层次关系探索](../../requirements/REQ-3-hierarchy/README.md)、[REQ-5 符号与图入口管理](../../requirements/REQ-5-symbol-management/README.md)、[REQ-8 语言支持](../../requirements/REQ-8-language-support/README.md)、[REQ-9 项目本地配置与符号过滤](../../requirements/REQ-9-project-configuration/README.md)。

rust-analyzer 的进程模型、冷索引原因与未来复用路线单独记录在 [rust-analyzer 生命周期与索引复用设计](rust-analyzer.md)，避免把语言专用取舍混入通用 LSP actor 说明。

不同语言服务器的符号展示字段不能用同一套启发式解释。workspace symbol 与 call hierarchy 的命名边界、适配器选择、Rust `类型名::方法名` 和 Pyrefly `Class.method` 规则记录在 [LSP 符号命名适配](lsp/README.md)。

## Provider 与 Client Handle

`LspProvider` 拥有语言服务器子进程、连接任务和关闭流程，不能克隆。`LspConfig::for_server` 把可执行文件转换为实际 stdio 命令；`server_name` 可显式选择语言适配 profile，适用于绝对路径或包装脚本。大多数 server 不加参数，Pyrefly profile 自动增加 `lsp` 子命令，再追加用户参数。LSP 专用 client 只持有 JSON-RPC actor 的发送端、canonical workspace root 和服务端 hierarchy 能力状态；`TreeSitterProvider` 拥有 grammar/query readiness 和共享的 single-flight 项目索引状态。Fetch 顶层的 `WorkspaceSymbolClient` / `HierarchyClient` 把实现收敛为可克隆的窄接口，TUI 不判断 provider 类型。`HierarchyClient::Hybrid` 可以在同一会话中保留 LSP 主后端与 Tree-sitter 后备，并按单次查询能力选择，而不是把整个会话强制绑定到较弱后端。

这样设计有两个原因：

1. 每次防抖结束后会产生一个异步定向查询，但查询任务不应拥有或关闭子进程。
2. 退出时仍由 `main` 统一执行 `shutdown` / `exit`，生命周期不会散落在 UI 任务中。

LSP workspace symbol 与 hierarchy 客户端复用同一个 actor 和语言服务器进程；Tree-sitter 两种客户端复用同一个惰性静态索引，都不会为不同请求类型重复建立后端状态。

LSP actor 的协议边界保持通用：它发送标准 initialize、initialized、workspace/document symbol 和 call/type hierarchy 请求，不把某个 server 的私有索引行为暴露给 App 或 TUI。`.cgraph.toml` 的 `[lsp]` 段负责选择 profile 名称、可执行命令、参数和项目文件后缀；内置 profile 为 rust-analyzer、clangd 和 Pyrefly。部分 profile 仍保留受限 bootstrap 文档等兼容逻辑；文件后缀决定 profile 可以选择哪些项目文档，初始化选项和 bootstrap 策略开关尚未开放为通用 TOML 字段。新增语言服务器时，优先实现 profile/adaptor，而不是在通用 actor 中增加按程序名分支。

## VS Code 式 workspace symbol 查询

`WorkspaceSymbolClient::query` 把当前完整文本直接交给 server，不尝试枚举完整索引，也允许空字符串。TUI 负责与 VS Code 相同的约 200 ms 防抖节奏；Fetch 层只负责一次查询的协议语义。server 返回后先删除完全相同的符号，再按 URI 做项目范围过滤：只有能够转换为本地文件路径且位于 canonical workspace root 下的符号才保留。

rust-analyzer 默认的 workspace symbol 只搜索类型。cgraph 会把 `scope=workspace` 与 `kind=all_symbols` 递归合并进 initialization options，同时保留调用方的其他设置；`workspace/configuration` 也返回相同策略。cgraph 不覆盖默认 limit，因为 rust-analyzer 的 128 项默认值就是为“客户端随过滤文本重新查询”的模式设计的。服务端差异必须收敛在 Fetch 层，App/TUI 不应知道 `#`、`*` 等 rust-analyzer 私有查询标记。

URI 过滤是对 server scope 的额外保护，确保依赖和工作区外路径默认不进入候选。虚拟文档和非文件 URI 当前同样会被排除；未来若支持远程 workspace，需要把范围判断抽象为 URI containment policy。

## 请求取消

JSON-RPC actor 为每个请求分配 id，并把 id 回传给等待响应的 future。future 持有一个取消守卫：正常收到响应时解除；若 TUI 因新输入或关闭弹窗而 abort task，守卫在 drop 时通过独立的无界 channel 通知 actor。actor 从 pending map 删除请求并写出 `$/cancelRequest`，因此无需在异步析构中等待锁或 I/O。

取消只能表达客户端不再需要结果，不能假定 server 一定停止计算。App 层的单调 request id 仍会拒绝迟到响应。actor 也只为仍在 pending map 中的 id 发送取消通知，避免正常完成后产生无意义的 `$/cancelRequest`。

## 为什么使用后台 JSON-RPC actor

LSP 并不是“发送请求后只等待对应响应”的简单协议。语言服务器可能在任意时刻发送通知或反向请求，例如 `workspace/configuration`。如果只在用户发起查询时读取 stdout，服务器可能等待配置响应，而客户端又等待查询结果，形成死锁。

当前实现分为：

```text
reader task ── incoming channel ──> JSON-RPC actor ──> pending request
client task ── command channel ──> JSON-RPC actor ──> server stdin
                                      │
                                      └── server request response
```

actor 是唯一写入 server stdin 的任务，因此消息不会交叉写坏；pending map 根据数值 request id 将乱序响应送回对应 oneshot channel。

## LSP 状态与进度通知

cgraph 在 initialize capabilities 中声明标准 `window.workDoneProgress`，并声明 rust-analyzer 的实验性 `serverStatusNotification`。JSON-RPC actor 始终读取通知，将协议细节归一化为 `LspStatusUpdate` 后通过独立无界 channel 交给 TUI；进度通知不会混入 workspace symbol 的请求/响应 channel。

标准 `$/progress` 可以包含多个并行 token。`LspProgressTracker` 保存每个活动 token 的标题、消息、百分比和最近更新时间，UI 展示最近更新的任务。end 只删除对应 token；仍有其他任务时继续展示其中最新的一项，最后一个任务结束后才发送 `Ready`。百分比的最终防御性截断在 TUI 映射层完成。

rust-analyzer `experimental/serverStatus` 的 `health=warning/error` 映射为相应错误等级；quiescent 状态在没有标准活动任务时映射为 `Ready`，否则不能覆盖仍在进行的 work-done progress。连接 actor 结束时无论正常或异常都会发送 `Disconnected`，并使 pending 请求失败。

该通道表达 server 主动报告的状态，而不是 cgraph 对索引是否完成的推断。不支持 progress 的 server 在 initialize 后可能一直显示 `Ready`；未来如需更强语义，应增加 provider capability/heartbeat，而不是根据一次搜索耗时猜测。

## 传输约束

- 使用标准 `Content-Length` 帧。
- client capabilities 只声明 UTF-16 position encoding；server 显式选择其他编码时拒绝会话，保证 State/IPC 的 character 含义唯一。
- 单条消息限制为 16 MiB，避免错误或恶意服务端造成无界分配。
- 支持响应、通知和常见服务端反向请求。
- 未实现的服务端请求返回 JSON-RPC `Method not found`，不静默伪造成功。
- LSP stderr 当前丢弃，以免破坏备用屏幕；后续应接入文件日志或内存诊断缓冲区。

## 统一 hierarchy 查询

`HierarchyQuery` 描述语义符号、call/type 模式和 incoming/outgoing 方向；`HierarchyResponse` 记录归一化孩子和数据来源。`HierarchyClient` 对精确位置执行标准两阶段请求；CLI 根没有位置时，先执行 workspace symbol 精确解析，同名候选不唯一则返回错误而不是任选一个重载。

call hierarchy 使用 prepare 后的 `CallHierarchyItem` 请求 incoming/outgoing calls；type hierarchy 使用 `TypeHierarchyItem` 请求 supertypes/subtypes。协议 item 的 `data` 在第二阶段请求中保留。`detail` 不是结构化容器字段，只能由对应 LSP adapter 解释；当前 rust-analyzer 会把方法标成 `Function` 并把签名放在 `detail`，所以 Rust adapter 额外按文件请求并缓存 document symbols，用标准 `containerName` 生成 `Type::name`，失败时保留原名。LSP hierarchy 的返回项与 workspace symbol 使用同一项目范围策略：默认只有 `file://` URI 且路径位于 canonical workspace root 下的项才进入 `HierarchyResponse`；`.cgraph.toml` 的 `[filters].workspace_only = false` 可以关闭这一范围过滤。这样 clangd 从项目函数返回 `/usr/include` 中的 `printf` 时，默认不会把未 `didOpen` 的系统头文件加入图，也不会在用户展开它时触发 `trying to get AST for non-added document`；项目内文件的协议错误仍原样反馈给消息 pager。Fetch 会按 `SymbolIdentity` 去重 provider 响应，App 在写入 incoming/outgoing 缓存前应用项目过滤并做防御性分支内去重；State 按层次类型与源码位置全局复用同一 `NodeId`，同时保留不同方向观察到的边。

call hierarchy 可以在 initialize result 中静态声明，也可以动态注册；type hierarchy 在 LSP 3.17 中只通过 `client/registerCapability` 注册。客户端将两项 `dynamicRegistration` 声明为 `true`，actor 按 registration id 追踪注册与注销。查询发出前必须检查当前能力，未声明的方法不能靠“试一次并接受 `-32601`”探测，因为这会把可预知的能力缺失污染为用户错误。

主程序会为可识别的 Rust、C、C++、Python 工作区同时初始化一个轻量 Tree-sitter hierarchy 后备。grammar/query 初始化不扫描项目；只有 LSP 未声明当前 hierarchy kind 时，`Hybrid` 才把该次查询交给 Tree-sitter 并惰性建立共享索引。workspace symbol 始终优先 LSP，已声明的 hierarchy 也仍由 LSP 处理。Tree-sitter 响应保留 `FetchSource::TreeSitter`，因此 UI 会明确提示语法级置信度。若没有可用的 Tree-sitter 语言，LSP client 在发送任何请求前返回清晰的 capability 错误。

Fetch 层不把方法不支持、连接错误或取消转换为空数组。成功空数组才表示该方向确实没有孩子。分支缓存和 request-id 竞争处理位于 App/State；后续显式刷新需要绕过缓存并保留仍存在孩子的实例状态。

## Tree-sitter 边界

Tree-sitter 不是 LSP 响应的逐字段替代品。`TreeSitterProvider::start` 只验证 grammar 与 tags query；第一次搜索或 hierarchy 请求触发独立的 single-flight 构建任务，通过 `tokio::spawn_blocking` 递归扫描项目。并发请求等待同一份结果；搜索防抖取消首个等待者时，项目扫描仍会完成并供后续请求复用，不会因快速输入反复扫描。成功结果进入共享只读索引，失败结果不缓存，后续操作可以重试。构建编号防止旧任务覆盖新的重试状态。

文件顺序排序后再解析，跳过隐藏目录、`target`、`node_modules` 和符号链接，因此候选和边顺序稳定且默认限定在项目内。

definitions 来自各 grammar 自带 tags query。Rust/Python 的 `reference.call` 直接复用 tags 捕获；C/C++ 使用额外的 `call_expression` query。调用引用先绑定到包含它的最内层函数，再按“同一类/impl、同一文件、全项目唯一”顺序解析目标；不能唯一绑定时不制造边。Rust `impl Trait for Type`、C++ `base_class_clause` 和 Python superclass 形成 parent → child 类型边，C 没有语言级继承关系。Rust/C++ 方法用 `Type::method`，Python 方法用 `Type.method`。

Tree-sitter 的 `Point.column` 是 UTF-8 字节偏移；索引层根据捕获点所在行的 UTF-8 前缀计算 UTF-16 code-unit 数，再构造公共 `SourceLocation`。这与 LSP 的强制 UTF-16 协商保持一致，避免 IPC 或节点身份在非 ASCII 源码中混用两种列坐标。

`FetchSource::TreeSitter` 是显式置信度边界。App 接收成功结果后在倒数第二行显示 `syntactic relations only; dynamic dispatch may be omitted` 并写入消息历史，因此成功空数组只表示索引中没有可唯一绑定的语法边，不被描述成完整语义证明。项目外调用、动态分派、宏展开、复杂 import/namespace 解析及歧义重载当前可能省略。
