# rust-analyzer 生命周期与索引复用设计

本文档记录 cgraph 使用 rust-analyzer 时的进程模型、首次 workspace symbol 查询较慢的原因，以及未来可能采用的索引复用方案。它是维护者设计文档，不是已经承诺的用户功能清单。

相关产品规范：[REQ-2-1 LSP 状态与进度](../../requirements/REQ-2-analysis-status/REQ-2-1-lsp-progress.md)、[REQ-5-2 异步查询生命周期](../../requirements/REQ-5-symbol-management/REQ-5-2-query-lifecycle.md)和[REQ-8 语言支持](../../requirements/REQ-8-language-support/README.md)。daemon 与编辑器代理仍是内部候选方案，未单独建立产品需求。

## 当前决策

当前阶段保持简单模型：每个 cgraph 进程启动并独占一个 rust-analyzer 子进程；TUI 退出时按 LSP 规范发送 `shutdown` 和 `exit`，随后回收子进程。

```text
cgraph process
├── App / TUI
├── JSON-RPC actor
└── rust-analyzer child process
```

同一次 cgraph 会话中，反复打开和关闭 `ac` / `at` 会复用相同的 rust-analyzer 及其内存索引。退出 cgraph 后，rust-analyzer 的内存分析数据库随进程销毁；下一次启动会重新创建。

rust-analyzer 当前不实现 LSP 3.17 的标准 type hierarchy 请求，也不会动态注册 `textDocument/prepareTypeHierarchy`。直接发送该方法会收到 JSON-RPC `-32601 unknown request`；这不是索引尚未完成，也不能通过等待或重启解决。cgraph 现在追踪 server hierarchy capability，Rust 的 workspace symbol 与 call hierarchy 继续使用 rust-analyzer，type hierarchy 则按单次查询回退到进程内的 Tree-sitter Rust 索引。该索引只在第一次需要回退时惰性扫描项目，不会启动第二个 rust-analyzer，也不会影响 rust-analyzer 的索引生命周期。

Tree-sitter 能确定 Rust `impl Trait for Type` 的 trait → type 关系，但不是 rust-analyzer 语义数据库的替代品；宏展开、复杂类型别名和其他非直接语法关系可能省略。回退结果带有独立来源，TUI 会显示 `syntactic relations only`，避免把成功空结果描述成完整语义结论。若未来 rust-analyzer 正式注册标准 type hierarchy，混合 client 会自动优先使用它，无需按版本号维护硬编码名单。

选择这个模型是因为它具备清晰的所有权和故障边界：

- 一个 cgraph 对应一个 LSP 会话，不需要协调多个客户端的配置和文档状态。
- request id、取消请求、server 反向请求和 shutdown 生命周期都只有一个所有者。
- 不依赖 rust-analyzer 未承诺稳定性的内部接口或私有传输协议。
- cgraph 崩溃或配置错误不会影响编辑器正在使用的 rust-analyzer。
- 当前实现可以继续用于 clangd、Pyrefly、pylsp 等其他 stdio LSP server，而不把主流程绑定到 Rust 专用 daemon。

代价是每次重新启动 cgraph 都会失去 rust-analyzer 的内存缓存，首次语义查询可能明显慢于已经打开一段时间的编辑器。

## 当前启动与关闭顺序

`main` 在进入备用屏幕前启动语言服务器：

```text
parse CLI
  -> select LSP configuration
  -> spawn rust-analyzer
  -> initialize / initialized
  -> create WorkspaceSymbolClient / hybrid HierarchyClient
  -> initialize TUI
  -> run event loop
  -> restore terminal
  -> shutdown / exit rust-analyzer
```

`initialize` 完成只表示 LSP 会话已经建立，不表示 Cargo workspace、build scripts、proc macros 或符号索引已经全部准备好。rust-analyzer 可以在 TUI 出现后继续后台索引，因此用户立即打开搜索框时仍可能遇到冷查询。

当前 `LspProvider` 由 `main` 独占，TUI 只获得可克隆的 `WorkspaceSymbolClient`。短生命周期搜索 task 可以取消请求，但不能关闭或替换整个 server。这一边界后续即使引入 daemon 也应保留。

## rust-analyzer 的官方进程模型

截至 2026-08-23，本机 rust-analyzer 1.98.0 与上游 master 的官方 LSP 入口都使用 `Connection::stdio()`。官方 CLI 没有稳定的 `--listen`、`--socket`、`--tcp`、`--daemon` 或“连接已有实例”选项。

这意味着普通 rust-analyzer 进程只有启动它的客户端持有 stdin/stdout 管道。cgraph 不能在进程启动后附加到 VS Code、Neovim 或其他编辑器拥有的实例。通过 `/proc/<pid>/fd/*` 强行写入也不能形成合法的第二条 LSP 连接：响应会回到原客户端，请求 id 可能冲突，多个写入者可能破坏 `Content-Length` 帧，`initialize`、配置、打开文档和 shutdown 状态也无法隔离。

未来升级 rust-analyzer 时应重新检查官方 CLI 和发布说明，但不能只根据版本号猜测是否可共享。即使上游增加网络监听，也必须确认它是否明确支持多客户端共享同一个分析数据库；“使用 socket 传输”和“支持多客户端 daemon”是两个不同能力。

## 哪些缓存能够复用

| 数据 | 跨 cgraph 进程复用 | 说明 |
| --- | --- | --- |
| Cargo `target/` 构建产物 | 可以 | 由 Cargo 和 rustc 管理，不等同于 IDE 语义索引 |
| 操作系统页缓存 | 可能 | 重启后读取源码和依赖通常比完全冷磁盘快，但不可作为正确性或延迟保证 |
| rust-analyzer Salsa 数据库 | 不可以 | 主要语义查询状态保存在 rust-analyzer 进程内存中 |
| module/crate symbol index | 不可以 | 属于当前 Salsa 数据库；进程退出后丢失 |
| 同一 cgraph 会话内的 symbol index | 可以 | 相同 rust-analyzer 进程会复用已经计算的 tracked query |
| 编辑器现有 rust-analyzer 的索引 | 不可以直接复用 | stdio 会话由编辑器独占，除非编辑器主动代理查询 |

`rust-analyzer prime-caches` 是独立批处理子命令。它可以预热自己的进程以及间接改善文件系统/Cargo 缓存，但不能把该进程的 Salsa 数据库移交给随后启动的 LSP server，因此不能作为 cgraph 的持久索引文件使用。

## 为什么第一次 workspace symbol 查询较慢

cgraph 的搜索框采用约 200 ms 防抖。打开弹窗后如果没有继续输入，会发送空字符串 `workspace/symbol`；输入按空白拆分后只把第一项发送给 server，后续项由客户端在候选的名称、容器和路径上模糊筛选。cgraph 为 rust-analyzer 设置 `kind=all_symbols` 和 `scope=workspace`，以便 call 搜索能够找到函数，同时默认排除依赖。

rust-analyzer 对没有 module path 限制的 workspace symbol 查询会取得所有 local roots 中的 crates，并并行建立各 crate/module 的 `SymbolIndex`，随后使用 FST 完成名称匹配。空字符串的模糊子序列自动机会匹配几乎所有名称，因此会遍历大量符号。

rust-analyzer 默认响应上限为 128，但上限是在 `world_symbols` 已经建立索引并产生匹配集合之后应用。降低响应项数不能消除首次索引成本。`all_symbols` 还会让函数、常量和模块等非类型项目进入搜索范围，比默认 `only_types` 做更多工作。

在当前 cgraph 仓库进行过一次诊断测量：新 rust-analyzer 进程的空查询命令总耗时约 19.1 秒，`main` 定向查询总耗时约 13.7 秒。诊断示例包含固定 2 秒等待以及 server 启停时间，因此这些数字不能当作纯查询基准；它们只说明冷启动和首次索引占据主要成本。性能判断应记录机器、rust-analyzer 版本、工作区规模、查询文本以及索引是否已经预热。

## 当前模型下的改进方向

以下改进不要求跨 cgraph 进程共享 rust-analyzer，可以优先考虑。

### 1. 可观测启动阶段

基础状态展示已经实现：cgraph 声明 work-done progress 和 rust-analyzer server status capability，JSON-RPC actor 归一化 `$/progress` 与 `experimental/serverStatus`，TUI 最底栏右侧可区分 LSP 已连接、后台工作、warning/error 和断开，左侧同时保留快捷键。搜索弹窗仍独立显示单次 `workspace/symbol` 的防抖与请求状态，因此不会再用一次请求的 Loading 推测整个 server 是否就绪。

仍需记录至少四段结构化耗时：进程启动、`initialize`、索引/缓存预热、第一次 `workspace/symbol`。当前 server stderr 被丢弃，也没有持久性能记录，不利于区分 Cargo metadata、build script、proc macro 和符号索引瓶颈。后续应支持显式日志文件，并对敏感路径和 server 输出设置清晰的保留策略。部分 server 不发送 progress，因此 UI 的 `Ready` 只能表示当前没有已知活动任务，不能作为索引完成的强保证。

### 2. 调整空查询策略

当前打开弹窗会按 VS Code 模式安排空查询。对于随用随启的 cgraph，空查询很容易成为第一次最昂贵的操作。未来可以提供产品选项：

- 保持空查询，允许打开弹窗后浏览 server 返回的前 128 项；
- 空输入只显示 `Type to search…`，用户输入非空文本后才请求 server；
- 仅在 rust-analyzer 报告索引就绪后执行空查询。

第二种通常能获得最直接的首次交互改善，但会有意偏离当前 VS Code 式空查询行为。改变默认值前需要真实项目基准和用户体验验证，不能仅依据当前小仓库的一次测量。

### 3. 同进程预热

可以在 rust-analyzer 初始化后、用户打开弹窗前安排低优先级预热，但预热必须发生在将被实际查询复用的同一个进程中。直接发送空 `workspace/symbol` 虽能建立 symbol index，却可能抢占 CPU、延迟 TUI 或与用户的真实查询竞争；更理想的是依据 server progress 判断时机，并允许取消。

### 4. 减少不需要的分析能力

禁用 build scripts 或 proc macros 可能改善某些项目的启动时间，但会降低 cfg、生成代码和符号位置的准确性。这类选项只能作为显式配置和诊断手段，不能为了性能默认牺牲 hierarchy 正确性。

## 长期方案一：cgraph workspace daemon

如果跨启动复用成为核心需求，推荐由 cgraph 自己提供 workspace daemon，而不是修改或强行附加 rust-analyzer：

```text
cgraph TUI 1 ─┐
cgraph TUI 2 ─┼── Unix socket ──> cgraph workspace daemon ── stdio ──> rust-analyzer
Neovim client ┘
```

daemon 是唯一 LSP 客户端，长期持有 rust-analyzer 和 Salsa 索引；TUI 退出只关闭前端连接。现有 IPC 模块可以作为基础，但实现前必须解决以下问题。

### 实例身份

daemon key 至少包含 canonical workspace root、LSP 可执行文件、参数、影响分析的 initialization options 和 cgraph 协议版本。只按目录复用可能把不同 toolchain、features 或 server 配置错误地合并。

### 请求路由与取消

每个前端 request id 必须映射到独立的内部 LSP request id。客户端断开或文本变化时，daemon 负责引用正确的内部请求发送 `$/cancelRequest`。server 通知、反向请求和错误不能广播给不相关工作区或泄漏到其他用户会话。

### 文档状态

workspace symbol 主要依赖磁盘文件，但未来 hierarchy、跳转和编辑器未保存内容会引入 `didOpen` / `didChange` 所有权。daemon 必须定义磁盘真相、编辑器 overlay 和多个客户端冲突时的策略，否则共享进程可能得到不可解释的语义结果。

### 生命周期与安全

- socket 放在用户运行时目录，并使用仅当前用户可访问的权限；
- 使用锁或原子 bind 保证一个 daemon key 只有一个实例；
- 只清理能够证明属于 cgraph 且已经失效的 socket；
- 设置可配置 idle timeout，而不是最后一个 TUI 退出就立即关闭；
- rust-analyzer 崩溃时重启并向客户端报告缓存失效，不能返回伪装成成功的空结果；
- cgraph 或协议版本不兼容时拒绝复用，并提供可诊断的升级路径。

### 推进顺序

1. 在已有 server progress 窗口上补充查询分段耗时和日志，建立冷/热基准。
2. 固化 IPC 帧格式、能力握手、request id 和取消语义。
3. 实现显式 `cgraph daemon --workspace ...`，暂不自动后台化。
4. 让普通 cgraph 客户端选择连接 daemon 或继续使用独立 LSP。
5. 验证崩溃恢复、配置指纹、socket 安全和 idle timeout 后，再考虑自动发现/启动。

独立进程模式必须长期保留为回退路径，便于调试 daemon、隔离配置问题以及支持不适合常驻的语言服务器。

## 长期方案二：编辑器代理

VS Code 或 Neovim 插件可以使用编辑器已经建立的 LSP client 执行 workspace symbol/hierarchy 请求，再通过 cgraph IPC 返回结果。这能够间接复用编辑器的热索引和未保存文档状态，但会让 cgraph 依赖编辑器在线，并需要为不同编辑器实现适配层。

编辑器代理应被建模为另一种 Fetch provider，而不是让 cgraph 直接读写编辑器所拥有的 rust-analyzer stdio。provider 必须报告能力、工作区、配置和结果来源，cgraph 才能决定是否回退到自己的 LSP。

## 长期方案三：上游正式共享能力

如果未来 rust-analyzer 官方提供稳定 daemon、持久索引或多客户端协议，cgraph 可以增加 capability detection。采用前必须验证：

- 能否按 workspace 和配置隔离分析数据库；
- 是否真正支持多个并发客户端，而不只是把单连接从 stdio 换成 socket；
- 未保存文档、shutdown、配置更新和 server notifications 的语义；
- 协议和持久数据格式是否属于稳定接口；
- 连接失败或版本不兼容时能否安全回退到独立进程。

不能依赖进程扫描、固定端口、私有环境变量或未承诺兼容性的内部数据库格式。

## 决策触发条件

在以下信息齐备前，不应直接把 workspace daemon 设为默认：

- 多个真实中大型 Rust 项目的冷启动、首次查询和热查询基准；
- 用户是否更在意首次空结果浏览，还是输入后快速定向命中；
- 常驻 rust-analyzer 的典型内存成本与合理 idle timeout；
- IPC 安全、崩溃恢复和配置变化测试；
- call/type hierarchy 对未保存文档状态的实际要求。

当前结论是：先保持每个 cgraph 启动自己的 rust-analyzer；基础索引状态已经可以观察，下一步完善分段耗时和日志。当跨启动冷查询成为稳定、可量化的主要瓶颈后，再按上述阶段实现可选 workspace daemon。
