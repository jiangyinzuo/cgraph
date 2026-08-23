# IPC 层设计

相关产品规范：[REQ-6 进程间通信](../../requirements/REQ-6-ipc/README.md)。当前文件中的协议和生命周期骨架属于实现准备，不代表需求已经部分交付。

IPC 层将通过 Unix domain socket 连接 Neovim 等客户端。当前只有 socket 配置和协议数据骨架，没有监听循环，也不会创建或删除 socket 文件。

未来 IPC 也可能承载 workspace daemon，让多个短生命周期 TUI 复用同一个 rust-analyzer；该方案的实例身份、请求路由和文档状态要求记录在 [rust-analyzer 生命周期与索引复用设计](../fetch/rust-analyzer.md)。当前决策仍是每个 ctree 启动自己的语言服务器。

## 方向

客户端到 ctree：

- 请求聚焦或新建 call/type 根节点。
- 后续可增加能力握手和状态查询。

ctree 到客户端：

- 用户双击节点时发送源码位置。
- 返回请求是否被接受；真正完成聚焦可能是异步事件。

## 协议骨架

`protocol.rs` 中的消息使用带 `type` 标签的 serde enum，并由 `Envelope` 携带协议版本和可选 request id。定义消息类型不等于已经选定传输帧；监听器实现前仍需决定：

- newline-delimited JSON 还是长度前缀帧；
- 单连接是否允许多个并发请求；
- 版本不兼容时如何响应；
- 客户端断开后的事件丢弃策略。

## Unix socket 生命周期 TODO

- socket 路径应位于用户运行时目录，并避免不同工作区互相冲突。
- bind 前只能删除确认属于 ctree 且已失效的 socket，不能无条件删除任意路径。
- 退出、panic 和收到终止信号时都应尽量清理 socket。
- 需要限制文件权限，防止其他用户注入打开文件或查询命令。
- 需要明确单实例策略：拒绝第二个服务端、复用现有实例，或为每个工作区创建实例。
- 所有来自 IPC 的路径和行列号都必须验证；客户端输入不能直接成为 shell 命令。

在这些规则确定前，`IpcServer` 保持不可启动的配置对象，避免一个看似可用但生命周期不安全的半成品服务端。
