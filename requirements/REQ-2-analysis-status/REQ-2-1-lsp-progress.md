# REQ-2-1：LSP 状态与进度

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-2` |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 需求

cgraph 应接收标准 LSP work-done progress，并在 rust-analyzer 可用时接收其 server status notification。状态至少包含 server 名称、阶段、消息和可选百分比。

## 验收条件

- initialize 声明 `window.workDoneProgress`。
- `$/progress` begin/report/end 被持续处理。
- 多个 token 并行时，一个任务结束不会错误切换为 Ready。
- rust-analyzer warning/error/quiescent 被映射到统一状态。
- 连接结束显示 Disconnected，百分比显示值限制在 0–100。

## 实现证据

- actor 与 progress tracker 位于 `src/fetch/lsp.rs`。
- 协议跟踪和 rust-analyzer 状态均有单元测试。
