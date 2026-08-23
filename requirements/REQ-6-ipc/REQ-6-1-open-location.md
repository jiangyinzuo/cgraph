# REQ-6-1：从节点跳转编辑器

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-6` |
| 状态 | `Implemented` |
| 优先级 | `P2` |
| 目标版本 | `0.1` |

## 需求

用户双击具有源码位置的节点时，cgraph 向已连接客户端发送文件 URI、行和列，使编辑器打开并跳转到对应位置。

## 验收条件

- 没有源码位置的节点不会发送伪造位置。
- 协议明确 LSP 零基行列与用户界面一基行号的边界。
- 多个客户端连接时的目标选择或广播规则在实现前确定。
- 发送失败不破坏 TUI 会话，并提供可诊断状态。

## 当前实现与证据

- 用户通过 `--ipc-socket <PATH>` 显式启动服务端；父目录必须已经存在，便于调用方选择自己的私有 runtime 目录和实例名称。
- 同一节点在 500 ms 内完成两次未拖拽点击时发送事件；provisional 节点或缺少完整 URI、行、列的位置只显示提示，不发送事件。
- `open_location` 使用协议版本 `1`、空 request id、零基行号和零基 UTF-16 character。cgraph 的 LSP 客户端只协商 UTF-16，Tree-sitter 字节列在进入公共状态前转换为 UTF-16。
- 所有当前已连接客户端都会收到事件。每客户端使用容量 16 的独立队列；断开或持续阻塞的客户端被移除，不阻塞 TUI 或其他客户端。
- socket 权限是 `0600`。陈旧文件只有在类型为 Unix socket、ownership marker 的 device/inode 完全匹配且连接明确返回 `ConnectionRefused` 时才会清理；退出时也只删除本实例 inode。
- 单元/集成测试覆盖协议形状、双击与无位置节点、多客户端广播、无客户端错误、权限、陈旧 socket、普通文件保护和 replacement socket 保护。

用户接入方式见[编辑器联动](../../docs/editor-integration.md)，实现细节见 [IPC 层设计](../../src/ipc/README.md)。
