# IPC 层设计

相关产品规范：[REQ-6 进程间通信](../../requirements/REQ-6-ipc/README.md)。cgraph → 编辑器的源码跳转和客户端 → cgraph 的 anchor 聚焦都已经实现。

IPC 层通过 Unix domain socket 连接 Neovim 等客户端。调用方用 `--ipc-socket <PATH>` 显式选择实例路径；不传选项时不创建 socket。显式路径比隐式 workspace hash 更容易被编辑器可靠发现，也避免在还没有实例注册表时猜测多 workspace / 多进程路由。

未来 IPC 也可能承载 workspace daemon，让多个短生命周期 TUI 复用同一个 rust-analyzer；该方案的实例身份、请求路由和文档状态要求记录在 [rust-analyzer 生命周期与索引复用设计](../fetch/rust-analyzer.md)。当前决策仍是每个 cgraph 启动自己的语言服务器。

## 方向

客户端到 cgraph：

- `focus_symbol` 请求按 call/type、符号名和可选精确位置聚焦或新建 anchor。
- 每个请求必须带 request id，并获得 `accepted` 或结构化 `error`。
- 后续可增加能力握手和状态查询。

cgraph 到客户端（已实现）：

- 用户双击节点时发送源码位置。
- 以事件广播，不等待编辑器确认是否真正完成打开。

## 帧与坐标

传输使用 newline-delimited JSON。每帧由 `Envelope` 携带 `version`、可选 `request_id` 和带 `type` 标签的 payload；序列化字符串内的换行会被 JSON 转义，因此物理换行始终是帧边界。`open_location` 是无请求对应的事件，`request_id = null`；客户端请求必须携带非空 `u64` id，响应原样复用它。

reader 使用 `BufReader::take(MAX_FRAME_BYTES + 1)` 在反序列化前执行 1 MiB 上限，避免攻击者用未结束的行造成无界分配。物理帧必须以换行结束；超大或断帧错误发送后关闭该连接。合法大小的帧依次经过 JSON envelope、协议版本、request id 和 typed payload 验证。版本不兼容不会尝试兼容性猜测；如果 envelope 已经提供 id，错误响应保留该 id。

公共 `SourceLocation` 统一使用 LSP 坐标：line 和 UTF-16 character 都从零开始。LSP 初始化只声明 UTF-16；server 选择其他编码会被拒绝。Tree-sitter `Point.column` 是 UTF-8 字节数，索引层根据源码行前缀转换为 UTF-16 code units 后才建立 `SourceLocation`。这样 IPC 不需要携带 provider 类型，Neovim adapter 只需把行号加一并把 UTF-16 character 转成 byte column。

## 广播与背压

`IpcServer` actor 同时接受连接、接收 TUI 事件和处理断开通知。每个客户端有独立的容量 16 channel 和 writer task；一次事件只序列化一次，再以 `Arc<[u8]>` 发给全部客户端。队列已满或 sender 已关闭时，该客户端从广播集合移除，因此慢客户端不会阻塞 Crossterm 同步事件循环，也不会拖住其他编辑器。

TUI sender 在入队前读取原子连接计数。无客户端、位置不完整或 server actor 已结束都会返回可诊断错误，TUI 更新倒数第二行摘要并写入统一消息历史；连接数是提示性快照，真正断开仍由各 writer 独立处理。

## 入站请求与响应

每个连接拆成一个 reader 和一个独占写半端的 writer。reader 不直接修改 `App` 或图，而是把已验证请求与 `IpcResponder` 送入全局容量 64 command channel。TUI 每轮渲染前 drain 已到达的 command，在事件循环线程调用 `App::focus_symbol`，再通过 responder 把 `accepted` / `error` 放回该客户端的 writer 队列。事件和响应因此共享同一条串行 NDJSON 输出流，不会由多个 task 并发写坏帧。

连接 reader 结束时，supervisor 先通知 server actor 移除该客户端的广播 sender，再等待 writer 排空已经生成的错误响应。如果直接取消 writer，超大帧等“发送错误后关闭”的路径会让客户端只读到 EOF，这是测试曾捕获的真实竞态。每客户端容量 16 的 writer 队列同时隔离慢消费者；command channel 有界则避免客户端以大量合法请求无限占用内存。

`App::focus_symbol` 是 UI 无关的语义边界。精确位置沿用 RelationGraph 的 resolved key，与搜索结果按 hierarchy kind、URI、line、character 去重；无位置时只复用同 kind 的唯一同名节点，没有候选则建立 provisional anchor，多候选返回 ambiguous。成功后 App 固定、选择节点并重置 viewport，TUI 仅负责 footer 文案和响应转发。`accepted` 不启动或等待 hierarchy 查询。

## Unix socket 生命周期

- 父目录必须由调用方预先创建，必须是真实目录而非符号链接，且不能允许 group/other 写入；用户文档推荐 `$XDG_RUNTIME_DIR` 下权限为 `0700` 的目录。这个约束把 bind、chmod 和 marker 创建之间的路径竞争限制在同一用户安全边界内。
- bind 后 socket 权限立即收紧到 `0600`，并创建同权限的 `.cgraph-owner` marker。marker 记录协议 magic、socket device 和 inode。
- 已有普通文件或符号链接绝不删除。同一路径已有活跃 cgraph 时拒绝第二个服务端。已有 socket 只有在 marker 身份完全匹配、同步 connect 明确得到 `ConnectionRefused`，且 unlink 前 inode 复核仍一致时才判为本项目的陈旧实例；权限错误和其他 I/O 错误都拒绝启动。
- `SocketGuard` 退出时重新比较 marker 与 inode。路径仍属于本实例才删除 socket；若路径已被 replacement socket 占用，只清理仍属于旧实例的 marker，不碰新 socket。
- `main` 在终端恢复后显式 shutdown actor；普通错误、unwind drop 和 runtime task abort 也会触发 guard 清理。`SIGKILL` 无法执行用户态清理，由下次启动的陈旧检测恢复。

## 后续边界

当前协议没有 capability handshake、实例发现、鉴权扩展、状态订阅或批量请求。私有父目录和 `0600` socket 把访问限制在本机用户安全边界内，但客户端仍应把路径视为可信配置。若未来增加其他 request 类型，应继续通过 typed protocol → 有界 command channel → App 状态迁移，而不能从 socket task 直接访问 Ratatui widget 或 `RelationGraph`。还需要补充真实 Neovim/PTY 端到端测试，以及多客户端高负载下的公平性和响应顺序压力测试。

IPC 未来也可能承载 workspace daemon，让多个短生命周期 TUI 复用同一个 rust-analyzer；该方案记录在 [rust-analyzer 生命周期与索引复用设计](../fetch/rust-analyzer.md)，不与当前编辑器事件 socket 混为同一承诺。
