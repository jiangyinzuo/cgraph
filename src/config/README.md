# 项目本地配置设计

相关产品规范：[REQ-9 项目本地配置与符号过滤](../../requirements/REQ-9-project-configuration/README.md)。

`config` 模块只负责定位 workspace 根目录的 `.cgraph.toml`、安全创建最小模板、校验 schema，并生成 UI 与 provider 无关的配置值。启动和 `ec` 返回后都走同一个严格 loader；本模块不监控文件变化、不启动编辑器，也不直接刷新图。

过滤实现位于 `filter.rs`。`FilterConfig` 保留配置文件中的有序 `Vec<FilterRule>`；每个规则明确是 `FilePath` 或 `Symbol`，运行时再投影为 App/LSP 使用的窄过滤器，避免两个列表在解析阶段失去原始顺序。

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
# Rules are applied from top to bottom; # marks a symbol rule.
rules = ["#*::into", "!#Option::into", "**/generated/**", "!src/generated/keep.rs"]
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

`filters.workspace_only` 控制初始的项目范围，默认值为 `true`。它也可以用路径规则中的特殊占位符 `<workspace>` 表达：该占位符匹配 workspace 外部路径，因此普通规则会隐藏外部 URI，`!<workspace>` 可以在后续规则中重新放行。`workspace_only = false` 时初始保留外部路径，仍可用 `<workspace>` 主动排除它们。clangd hierarchy 查询会按需读取目标文件并发送标准 `didOpen`，但项目外节点能否继续展开仍取决于语言服务器的 compile command。

`filters.rules` 是单一有序规则列表。普通规则匹配文件路径；以 `#` 开头（或以 `!#` 开头）的规则匹配完整符号名。普通规则排除匹配项，前缀 `!` 的规则重新包含匹配项；没有命中的项目保留，最后一个命中的规则决定结果。`#<all>` 是匹配所有符号的特殊占位符，常用于“全部隐藏后只放行少数名称”的配置。符号 `*` 匹配任意数量字符（包括限定名分隔符），因此 `#*::is_some` 能过滤任意类的方法。规则大小写敏感。

路径规则使用 workspace-relative 文件路径，统一使用 `/`。其中 `*` 不跨目录分隔符，`**` 可以跨任意目录层级；不含 `/` 的模式会匹配任意目录中的文件名。路径规则同样支持从前往后的普通排除与 `!` 重新包含，并支持 `<all>` 和 `<workspace>` 占位符。

规则类型不会切断顺序语义：路径规则排除的候选可以被更晚的符号规则重新包含。例如 `rules = ["<workspace>", "!#printf"]` 默认只保留项目内候选，同时允许外部 `printf`。因此运行时必须以候选的“完整名称 + URI”共同执行原始 `Vec<FilterRule>`，不能只把两种规则独立求值后再做布尔与运算。

加载时会去掉每项首尾空白并保留规则顺序；空字符串、裸 `!`、未知字段和错误类型都会使启动失败，并在错误中包含配置文件路径。不存在 `.cgraph.toml` 等同于空配置。匹配器使用线性空间动态规划，不会因多个通配符对长限定名或路径产生指数级回溯。

`filters.rules` 在 App 接收归一化结果后仍会再次检查，LSP provider 也会在 hierarchy/document 请求前执行路径范围过滤。这样 Tree-sitter 与 LSP 的搜索和邻接节点使用同一规则。用户显式通过 CLI 创建的 anchor 不受符号名过滤：规则只减少可发现候选和新加载的邻接节点，不应让一个明确请求悄悄消失。

`ProjectConfig::create_if_missing` 使用 `create_new` 写入最小有效模板。文件在检查与创建之间由其他进程生成时，只接受 `AlreadyExists`，绝不截断竞态中的用户内容。TUI 选择编辑器、管理终端和处理退出状态；App 替换过滤器后为所有已加载或正在刷新的可达分支生成新 request id。这个分工保证无效重载能保留旧配置，也保证编辑期间完成的旧查询不能覆盖新规则结果。

## 后续扩展

- 支持按 hierarchy kind、容器或源码路径缩小规则范围。
- 在确有需求时增加 `?`、字符组或显式 regex；当前故意只支持可预测的 `*`。
- 如果未来需要自动监控文件，复用同一严格 loader 和全图 refresh 边界，并对连续保存进行防抖。
