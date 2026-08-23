# REQ-6-2：外部 hierarchy 查询

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-6` |
| 状态 | `Implemented` |
| 优先级 | `P2` |
| 目标版本 | `0.1` |

## 需求

外部客户端可以发送 call/type 和目标符号。若该语义节点已在画布图中，cgraph 将其固定为 anchor 并居中；否则创建新的节点和 anchor。

## 验收条件

- 请求明确 hierarchy kind、符号身份和可选源码位置。
- request id 对应成功或结构化错误响应。
- 外部请求与本地搜索使用相同的语义去重规则。
- 协议版本不兼容时拒绝请求，不静默猜测格式。

## 当前实现

客户端通过同一个 Unix socket 发送不超过 1 MiB 的 newline-delimited JSON `focus_symbol` 请求。请求必须携带非空 `request_id`、call/type hierarchy kind、非空符号名和可选位置；位置一旦出现，就必须包含非空 `file://` URI 以及完整的零基 line、UTF-16 character。非法 JSON、未知 payload、缺失 request id、超大帧和不兼容版本都会得到结构化 `error`，能解析出 request id 时响应会原样携带它。

socket reader 只负责验证和入队。容量为 64 的 command channel 把请求交给 TUI 事件循环，后者调用 UI 无关的 `App::focus_symbol`，不从连接任务直接修改 Ratatui widget 或关系图。有精确位置时复用与本地搜索相同的 resolved identity；没有位置时，同名同 hierarchy kind 的唯一节点被复用，没有候选则创建 provisional anchor，多候选则返回要求精确位置的 ambiguous 错误。成功会固定、选中并居中该节点，然后返回相同 request id 的 `accepted`；它不隐含 hierarchy 已经加载。

## 验收证据

- `src/ipc/tests.rs` 使用真实 Unix socket 验证请求路由、相同 request id 的响应、版本拒绝、1 MiB 上限和错误响应排空。
- `src/app.rs` 验证 resolved identity 复用、无位置唯一匹配、provisional 创建、同名歧义和非法位置拒绝。
- `src/tui/mod.rs` 验证外部 command 经 App 修改状态，并返回 `accepted` 或结构化 `error`。
- 用户协议和 Neovim 双向示例见 [`docs/editor-integration.md`](../../docs/editor-integration.md)。
