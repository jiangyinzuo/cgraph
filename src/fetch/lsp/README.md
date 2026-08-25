# LSP 符号命名适配

LSP 对符号名称的标准化程度并不一致。`workspace/symbol` 的 `containerName` 明确表示符号所属容器，但 `CallHierarchyItem.detail` 只是供界面展示的任意文本；clangd、rust-analyzer、Pyrefly 和语言服务器包装器可能返回完全不同的格式。cgraph 因此不能用一个字符串启发式同时解释所有语言。

`SymbolNameAdapter` 是 Fetch 层内部的适配边界。provider 完成 initialize 后，同时参考启动程序的文件名与 `InitializeResult.serverInfo.name` 选择适配器。后者让 `lsp-wrapper rust-analyzer` 一类启动方式仍能选择 Rust 规则。App、State 和 TUI 只接收适配后的显示名，不感知 server 私有格式。

## 通用规则

- 普通函数保持 server 返回的名称。
- 已经包含 `::` 的名称不重复限定。
- 标准 workspace symbol 的 method/constructor 可以使用安全的 `containerName`，界面统一显示为 `Container::method`。
- 通用 call hierarchy 不解释 `detail`。该字段没有结构化协议契约，把它直接拼进节点名可能产生文件路径、函数签名或服务器提示文本。

## rust-analyzer 规则

Rust 没有 class 关键字；产品中的“类名”在 Rust 中指方法所属的类型或 trait。rust-analyzer 1.98 的 call hierarchy 有两个重要差异：方法和关联函数可能都标成 `Function`，而 `CallHierarchyItem.detail` 是 `pub fn run(&self)` 一类签名，不含所属类型。只解析 call hierarchy item 无法正确得到类名。

Rust adapter 因此采用两步归一化：先取得 call hierarchy items，再按其中的文档 URI 请求标准 `textDocument/documentSymbol`。它用名称和 selection position 匹配对应 document symbol，读取 `containerName`（例如 `impl Worker`），最后生成 `Worker::run`。每个 URI 的 document symbols 在当前 LSP 会话中缓存，包括失败后的空结果；同一文件中的多个节点或后续展开不会重复请求整份文档符号。

容器文本解析支持：

```text
impl Worker                         -> Worker::run
impl<T> Worker<T>                   -> Worker::run
impl Job for worker::Worker         -> Worker::run
impl<T> Job<T> for Worker<T>        -> Worker::run
<worker::Worker as Job>             -> Worker::run
trait Job                           -> Job::run
```

trait impl 优先显示具体 self type，而不是 trait 名；模块路径和泛型实参被移除，使节点稳定显示为 `类型名::方法名`。这也让 `.cgraph.toml` 中的过滤规则不依赖模块搬迁或泛型参数名称。

## Pyrefly

Pyrefly 的可执行文件通过 `pyrefly lsp` 进入 stdio LSP 模式，initialize 的 server name 是 `pyrefly-lsp`。命令适配按可执行文件名增加子命令；名称适配同时检查配置路径和 server name，因此包装器仍能在 initialize 后选择正确方言。

Pyrefly 的 lazy index 不会仅由 initialize 和 `workspace/symbol` 建立。initialize 完成后，provider 因此会确定性查找一个项目内 `.py` 文件，读取后发送一次标准 `textDocument/didOpen`；shutdown 前发送对应的 `didClose`。这个文档只用于触发 server 的 workspace index，cgraph 不对它发送 `didChange` 或 `didSave`。扫描跳过隐藏目录、`target`、`node_modules`、`__pycache__`、虚拟环境和符号链接，并限制为 10,000 个目录项及 4 MiB 文件；找不到合适文件时会话仍可使用，只是不声称后台索引已经完成。

Pyrefly 的 call hierarchy 把短方法名放在 `name`，把 `module.Class.method` 放在 `detail`。只有 Pyrefly adapter 会解释这个非结构化字段：方法和构造器提取最后两个合法 Python identifier，显示为 `Class.method`；模块函数保持短名，包含路径、签名或非法标识符的 detail 原样降级。workspace symbol 若提供 `containerName` 也使用点号，但 Pyrefly 当前主要返回顶层导出符号，cgraph 不枚举 server 没有返回的类成员。

没有源码位置的 CLI 根会先去掉最后一个 `::` 或 `.` 限定段，再进行 workspace-symbol 定位。因此 Python 输入可以使用惯用 `Class.method`，也兼容已有通用输入。Pyrefly 目前要求 workspace-symbol query 至少 3 个字符；客户端仍发送空文本和短 query，以保持 provider 接口与搜索生命周期一致，空响应不会被解释成能力缺失。

旧版或其他封装形式若直接在 `detail` 中提供 `impl` 描述，Rust adapter 仍可把它作为降级来源。解析保持保守：不完整的泛型、函数签名、文件路径和空文本都不会用于限定名称。document symbol 请求不支持、失败或没有匹配容器时，hierarchy 查询本身仍然成功并保留短方法名，避免命名增强破坏基础导航。

新增语言服务器时，应增加独立 adapter 分支和对应 fixture，不应放宽 Rust 解析器去猜测另一种协议方言。若某个 server 需要额外请求，也应在 adapter 边界内完成，并明确缓存与失败语义。
