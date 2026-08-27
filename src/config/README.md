# 项目本地配置设计

相关产品规范：[REQ-9 项目本地配置与符号过滤](../../requirements/REQ-9-project-configuration/README.md)。

`config` 模块只负责定位 workspace 根目录的 `.cgraph.toml`、安全创建最小模板、校验 schema，并生成 UI 与 provider 无关的配置值。启动和 `ec` 返回后都走同一个严格 loader；本模块不监控文件变化、不启动编辑器，也不直接刷新图。

`LspSettings` 是 `[lsp]` 段唯一的 Rust 配置模型，同时派生 TOML 的序列化和反序列化。它在加载后执行 command/name/extension 规范化；缺省模板通过同一个结构体序列化生成，避免模板字段与 loader schema 分叉。显式空 `name` 仍会报错；只有省略 `name` 时才从 command basename 推导。

## 当前 schema

```toml
[lsp]
# command and args are passed directly; no shell parsing is performed.
# name = "rust-analyzer"
# command = "rust-analyzer"
# args = []
# file_extensions = ["rs"]

[filters]
workspace_only = true
symbols = ["*::into", "Option::is_some", "*::Some"]
```

`[lsp]` 是可选的项目语言服务器配置。`name` 选择 server profile/语言方言；`command` 是实际可执行文件或路径，`args` 是按原样传递给它的参数列表；不能把整段 shell 命令写进一个字段。`file_extensions` 是 profile 可用于 bootstrap 扫描的项目文件后缀；加载器去掉可选前导点、统一为小写并去重，空数组、空后缀、路径和通配符会报错。省略时由 profile 补默认值：Rust 为 `rs`，clangd 为 `c/cc/cpp/cxx/h/hh/hpp/hxx`，Pyrefly 为 `py/pyi`。`name` 省略时从 command 的 basename 推导，适合 `/usr/bin/clangd` 这类路径；使用包装脚本时应显式填写真实 server name。省略 `[lsp]` 时，主程序按项目标志选择内置 profile：Rust 使用 `rust-analyzer`，C/C++ 使用 `clangd`，Python 使用 `pyrefly lsp`。CLI 的 `--lsp` / `--lsp-arg` 仍可作为一次性显式覆盖，优先级高于项目文件；`--no-lsp` 优先级最高。

配置命令的示例：

```toml
[lsp]
command = "clangd"
args = ["--background-index"]
file_extensions = ["c", "cpp", "h", "hpp"]
```

Pyrefly 仍只需填写 `command = "pyrefly"`，cgraph 会自动在参数最前面加入其标准 `lsp` 子命令；其他 server 不会被添加私有参数。配置文件通过 `ec` 修改后，过滤规则和项目范围会在当前会话刷新；LSP 可执行文件、参数或文件后缀的修改将在下一次启动 cgraph 时生效，避免在 TUI 中无序替换正在服务的 JSON-RPC 进程。

`filters.workspace_only` 控制 LSP workspace symbol 和 hierarchy 是否只保留位于当前项目根目录下的文件。默认值为 `true`；设为 `false` 后，客户端会保留语言服务器返回的项目外 URI。项目外节点是否能继续展开仍取决于语言服务器是否要求客户端先 `didOpen` 对应文档，例如 clangd 的系统头文件可能返回 `trying to get AST for non-added document`。

`filters.symbols` 是针对完整显示名的大小写敏感模式集合，匹配覆盖整个字符串；`*` 是唯一的通配符，表示任意数量字符。面向对象方法在 provider 给出容器信息时先规范化为 `Class::method`，再执行匹配，因此 `*::is_some` 能过滤任意类的 `is_some`，而 `Option::is_some` 只过滤指定类。普通函数仍使用自身名称。

加载时会去掉每项首尾空白并按首次出现顺序合并重复模式；空字符串、未知字段和错误类型都会使启动失败，并在错误中包含配置文件路径。不存在 `.cgraph.toml` 等同于空配置。通配符匹配使用动态规划而不是回溯，避免多个 `*` 对长限定名造成指数级耗时。

`filters.symbols` 发生在 App 接收已经归一化的查询结果之后，而不是 LSP 或 Tree-sitter 适配器中；`filters.workspace_only` 属于 LSP provider 的 URI 范围策略，因为它必须在请求 document symbol 之前阻止项目外 hierarchy item 进入适配器。Tree-sitter 索引天然只扫描项目内文件。用户显式通过 CLI 创建的 anchor 不受符号名过滤：规则只减少可发现候选和新加载的邻接节点，不应让一个明确请求悄悄消失。

`ProjectConfig::create_if_missing` 使用 `create_new` 写入最小有效模板。文件在检查与创建之间由其他进程生成时，只接受 `AlreadyExists`，绝不截断竞态中的用户内容。TUI 选择编辑器、管理终端和处理退出状态；App 替换过滤器后为所有已加载或正在刷新的可达分支生成新 request id。这个分工保证无效重载能保留旧配置，也保证编辑期间完成的旧查询不能覆盖新规则结果。

## 后续扩展

- 支持按 hierarchy kind、容器或源码路径缩小规则范围。
- 在确有需求时增加 `?`、字符组或显式 regex；当前故意只支持可预测的 `*`。
- 如果未来需要自动监控文件，复用同一严格 loader 和全图 refresh 边界，并对连续保存进行防抖。
