# REQ-6：进程间通信

| 字段 | 值 |
| --- | --- |
| 状态 | `Planned` |
| 优先级 | `P2` |
| 目标版本 | `TBD` |

## 目标

ctree 作为 Unix socket 服务端，与 Neovim 等外部客户端交换版本化请求，使 TUI 节点和编辑器位置可以双向联动。

## 子需求

| 子需求 | 状态 | 摘要 |
| --- | --- | --- |
| [REQ-6-1 从节点跳转编辑器](REQ-6-1-open-location.md) | `Planned` | 双击节点发送文件与位置 |
| [REQ-6-2 外部 hierarchy 查询](REQ-6-2-external-query.md) | `Planned` | 客户端请求定位或新建根 |

## 父需求验收

- Unix socket 生命周期、安全权限和陈旧 socket 清理规则明确。
- 消息带协议版本和 request id，错误可被客户端诊断。
- 外部消息通过 App command 修改状态，不直接操作 Ratatui widget。

## 当前实现与差距

已有 serde 协议 envelope 和序列化测试，但 socket listener、握手、命令路由和编辑器集成均未实现。协议骨架不代表用户已经可以连接，因此产品需求仍为 `Planned`。
