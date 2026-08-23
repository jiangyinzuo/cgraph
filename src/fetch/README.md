# Fetch 层设计

Fetch 层负责从外部分析能力获得符号和层次关系，再归一化为 State 层类型。当前实现了 LSP workspace symbol、标准 call/type hierarchy，以及 Rust、C、C++、Python 的 Tree-sitter grammar/tags-query 初始化。

相关产品规范：[REQ-2 分析后端状态](../../requirements/REQ-2-analysis-status/README.md)、[REQ-3 层次关系探索](../../requirements/REQ-3-hierarchy/README.md)、[REQ-5 符号与树管理](../../requirements/REQ-5-symbol-management/README.md)、[REQ-8 语言支持](../../requirements/REQ-8-language-support/README.md)、[REQ-9 项目本地配置与符号过滤](../../requirements/REQ-9-project-configuration/README.md)。

rust-analyzer 的进程模型、冷索引原因与未来复用路线单独记录在 [rust-analyzer 生命周期与索引复用设计](rust-analyzer.md)，避免把语言专用取舍混入通用 LSP actor 说明。

## Provider 与 Client Handle

`LspProvider` 拥有语言服务器子进程、连接任务和关闭流程，不能克隆。`WorkspaceSymbolClient` 与 `HierarchyClient` 只持有 JSON-RPC actor 的发送端和 canonical workspace root，可以安全克隆给短生命周期查询任务。

这样设计有两个原因：

1. 每次防抖结束后会产生一个异步定向查询，但查询任务不应拥有或关闭子进程。
2. 退出时仍由 `main` 统一执行 `shutdown` / `exit`，生命周期不会散落在 UI 任务中。

workspace symbol 与 hierarchy 客户端复用同一个 actor 和语言服务器进程，不为不同请求类型重复建立索引。

## VS Code 式 workspace symbol 查询

`WorkspaceSymbolClient::query` 把当前完整文本直接交给 server，不尝试枚举完整索引，也允许空字符串。TUI 负责与 VS Code 相同的约 200 ms 防抖节奏；Fetch 层只负责一次查询的协议语义。server 返回后先删除完全相同的符号，再按 URI 做项目范围过滤：只有能够转换为本地文件路径且位于 canonical workspace root 下的符号才保留。

rust-analyzer 默认的 workspace symbol 只搜索类型。ctree 会把 `scope=workspace` 与 `kind=all_symbols` 递归合并进 initialization options，同时保留调用方的其他设置；`workspace/configuration` 也返回相同策略。ctree 不覆盖默认 limit，因为 rust-analyzer 的 128 项默认值就是为“客户端随过滤文本重新查询”的模式设计的。服务端差异必须收敛在 Fetch 层，App/TUI 不应知道 `#`、`*` 等 rust-analyzer 私有查询标记。

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

ctree 在 initialize capabilities 中声明标准 `window.workDoneProgress`，并声明 rust-analyzer 的实验性 `serverStatusNotification`。JSON-RPC actor 始终读取通知，将协议细节归一化为 `LspStatusUpdate` 后通过独立无界 channel 交给 TUI；进度通知不会混入 workspace symbol 的请求/响应 channel。

标准 `$/progress` 可以包含多个并行 token。`LspProgressTracker` 保存每个活动 token 的标题、消息、百分比和最近更新时间，UI 展示最近更新的任务。end 只删除对应 token；仍有其他任务时继续展示其中最新的一项，最后一个任务结束后才发送 `Ready`。百分比的最终防御性截断在 TUI 映射层完成。

rust-analyzer `experimental/serverStatus` 的 `health=warning/error` 映射为相应错误等级；quiescent 状态在没有标准活动任务时映射为 `Ready`，否则不能覆盖仍在进行的 work-done progress。连接 actor 结束时无论正常或异常都会发送 `Disconnected`，并使 pending 请求失败。

该通道表达 server 主动报告的状态，而不是 ctree 对索引是否完成的推断。不支持 progress 的 server 在 initialize 后可能一直显示 `Ready`；未来如需更强语义，应增加 provider capability/heartbeat，而不是根据一次搜索耗时猜测。

## 传输约束

- 使用标准 `Content-Length` 帧。
- 单条消息限制为 16 MiB，避免错误或恶意服务端造成无界分配。
- 支持响应、通知和常见服务端反向请求。
- 未实现的服务端请求返回 JSON-RPC `Method not found`，不静默伪造成功。
- LSP stderr 当前丢弃，以免破坏备用屏幕；后续应接入文件日志或内存诊断缓冲区。

## 统一 hierarchy 查询

`HierarchyQuery` 描述语义符号、call/type 模式和 incoming/outgoing 方向；`HierarchyResponse` 记录归一化孩子和数据来源。`HierarchyClient` 对精确位置执行标准两阶段请求；CLI 根没有位置时，先执行 workspace symbol 精确解析，同名候选不唯一则返回错误而不是任选一个重载。

call hierarchy 使用 prepare 后的 `CallHierarchyItem` 请求 incoming/outgoing calls；type hierarchy使用 `TypeHierarchyItem` 请求 supertypes/subtypes。协议 item 的 `data` 在第二阶段请求中保留。method/constructor 若带有可用 `detail`，会规范化为 `Container::name`；路径、函数签名和缺失 detail 保留原名，避免把协议中的任意展示文本误当成类名。Fetch 会按 `SymbolIdentity` 去重 provider 响应，App 在写入 incoming/outgoing 缓存时再应用项目过滤并做防御性分支内去重；不同方向和不同树路径仍会创建不同 `NodeId`。

Fetch 层不把方法不支持、连接错误或取消转换为空数组。成功空数组才表示该方向确实没有孩子。分支缓存和 request-id 竞争处理位于 App/State；后续显式刷新需要绕过缓存并保留仍存在孩子的实例状态。

## Tree-sitter 边界

Tree-sitter 不是 LSP 响应的逐字段替代品。不同语言对调用、动态分派和类型继承的可判定程度不同。因此 Tree-sitter provider 后续需要显式报告结果置信度和不支持原因，而不是把“不知道”表示为空孩子列表。

当前 `TreeSitterProvider` 会检测工作区语言、初始化 parser grammar 与 tags query，并保留可执行的 parser。初始化只在没有可用 LSP 时作为 fallback 发生，结果通过通用 `AnalysisStatus` 显示。tags query 初始化成功不等于已经实现 workspace 范围遍历、候选归一化或 hierarchy 语义，所以搜索弹窗仍明确要求 LSP。
