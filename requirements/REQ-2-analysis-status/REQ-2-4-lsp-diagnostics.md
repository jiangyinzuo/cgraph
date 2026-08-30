# REQ-2-4：LSP 诊断与日志

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-2` |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.5` |

## 需求

用户在 workspace symbol 返回空结果或语言服务器行为异常时，应能确认实际连接的 server、workspace、索引引导文档、查询结果数量、过滤结果和 server stderr，而不需要在备用屏幕中直接混入日志。

## 验收条件

- LSP 初始化后把 server 版本、workspace、bootstrap URI 与 stderr 日志路径写入统一消息历史。
- `workspace/symbol` 最终为空时记录 query、server 返回候选数、项目过滤后数量、耗时和索引提示。
- 诊断消息不改变 LSP 的 Ready/Working/Error 连接状态，通过 `g<` pager 查看。
- 普通 cgraph 会话默认把 LSP stderr 追加到 `/tmp/cgraph-<server>-<pid>.log`，Unix 下新文件权限为 `0600`。
- `--lsp-log <PATH>` 和项目 `[lsp].log_file` 可以覆盖默认路径；相对项目配置路径按 workspace 解析。
- 内置 clangd 默认启用持久化 `--background-index`，显式 `--no-background-index` 等用户策略优先。

## 当前边界

日志保存的是 server stderr，不复制完整 JSON-RPC 消息，也不自动轮转。每个进程使用独立文件名以避免并发会话互相截断；临时文件的保留周期由操作系统或用户管理。
