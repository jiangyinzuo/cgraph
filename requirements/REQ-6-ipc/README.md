# REQ-6：进程间通信

| 字段 | 值 |
| --- | --- |
| 状态 | `Implemented` |
| 优先级 | `P2` |
| 目标版本 | `0.1` |

## 目标

cgraph 作为 Unix socket 服务端，与 Neovim 等外部客户端交换版本化请求，使 TUI 节点和编辑器位置可以双向联动。

## 子需求

| 子需求 | 状态 | 摘要 |
| --- | --- | --- |
| [REQ-6-1 从节点跳转编辑器](REQ-6-1-open-location.md) | `Implemented` | 双击节点发送文件与位置 |
| [REQ-6-2 外部 hierarchy 查询](REQ-6-2-external-query.md) | `Implemented` | 客户端请求定位或新建根 |

## 父需求验收

- Unix socket 生命周期、安全权限和陈旧 socket 清理规则明确。
- 消息带协议版本和 request id，错误可被客户端诊断。
- 外部消息通过 App command 修改状态，不直接操作 Ratatui widget。

## 当前实现

`--ipc-socket` 启动权限为 `0600` 的 Unix socket。节点双击把版本化的 UTF-16 零基源码位置以 NDJSON 广播给所有客户端；客户端也能用带 request id 的 `focus_symbol` 请求固定、选择并居中 call/type anchor。安全的陈旧 socket 判定、退出清理、慢客户端隔离、1 MiB 入站帧限制、版本拒绝、结构化响应和 footer 错误提示均有自动测试。

外部输入先经过 IPC reader 验证，再通过有界 App command channel 串行进入 TUI；连接任务不持有 widget 或图。精确位置复用本地搜索的 resolved identity，无位置请求只在同名同 kind 唯一时复用，否则创建 provisional anchor 或返回 ambiguous 错误。协议细节和 Neovim 示例见[编辑器联动](../../docs/editor-integration.md)。能力协商、多进程实例发现和常驻 workspace daemon 属于未来增强，不阻塞本需求。
