# REQ-9-1：配置与限定名过滤

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-9` |
| 状态 | `Implemented` |
| 优先级 | `P1` |
| 目标版本 | `0.1` |

## 需求

cgraph 启动时从 `--workspace` 根目录读取 `.cgraph.toml`。`filters.rules` 是从前往后的单一有序列表：普通规则排除匹配项，`!` 前缀重新包含，最后一个命中的规则获胜。普通规则按 workspace-relative 路径匹配；以 `#` 开头（或以 `!#` 开头）的规则按完整符号名匹配。符号 `*` 匹配任意字符；路径 `*` 不跨目录、`**` 可跨目录。`#<all>` 表示匹配全部符号，路径 `<workspace>` 表示排除 workspace 外路径。

`filters.workspace_only` 默认是 `true`，为路径过滤提供初始项目范围；设为 `false` 时初始保留项目外 URI，仍可由 `<workspace>` 规则主动排除。`!<workspace>` 可在后续规则中重新包含项目外路径。

项目配置也可以通过 `[lsp]` 的 `name`、`command`、`args` 和 `file_extensions` 选择语言服务器。`name` 选择 server profile，`command` 是实际可执行文件，`args` 是独立参数数组，`file_extensions` 是 profile 扫描项目文档时接受的后缀；省略后缀时 Rust 默认 `rs`，clangd 默认 `c/cc/cpp/cxx/h/hh/hpp/hxx`，Pyrefly 默认 `py/pyi`。省略整个段时使用 Rust 的 `rust-analyzer`、C/C++ 的 `clangd` 和 Python 的 `pyrefly lsp` 内置默认值。

面向对象方法在对应语言/provider 适配器能够确定容器时显示完整限定名，搜索结果与画布节点使用同一名称，并保留语言惯用分隔符：Rust/C++ 使用 `Class::method`，Python 使用 `Class.method`。Rust trait impl 优先使用具体实现类型，例如 `impl Read for Buffer` 显示为 `Buffer::read`。过滤规则以实际限定名为准，例如 Rust 的 `*::is_some` 或 Python 的 `*.run`。

## 验收条件

- 缺少 `.cgraph.toml` 时默认启用项目范围过滤，符号名模式保持为空。
- 无效 TOML、未知字段、错误类型和空模式在进入 TUI 前给出包含文件路径的错误。
- `file_extensions` 接受可选前导点并统一为小写、去重；空数组、空后缀、路径和通配符必须报错，C/C++ 默认值必须包含常见源文件与头文件。
- `*` 可以匹配零个或多个 Unicode 字符，其他字符按大小写精确匹配，模式必须覆盖完整显示名；路径中的 `*` 不跨 `/`，`**` 可以跨目录。
- `!` 规则覆盖前面的同类规则，规则顺序稳定且不去重。
- `#<all>` 可用于先隐藏全部符号再用 `!#` 放行少数名称；`<workspace>` 可用于路径范围控制。
- `#` 前缀明确区分符号规则和文件路径规则。
- 文件路径规则与符号规则共享同一覆盖顺序；`["<workspace>", "!#printf"]` 只为外部 `printf` 建立例外。
- 搜索候选与 incoming/outgoing hierarchy 子节点使用相同过滤器。
- 显式 CLI anchor 不被静默删除；配置只影响发现结果和后续加载的邻接节点。
- `filters.workspace_only = false` 时保留项目外 URI；clangd profile 会按需发送标准 `didOpen`，外部节点能否展开仍由语言服务器的 compile command 与能力决定。
- 限定方法名对应的节点宽度随文本扩展；终端足够宽时不截断类名或方法名。

## 实现证据

- `.cgraph.toml` 提供本项目的实际配置示例。
- `src/config/mod.rs` 覆盖缺省加载、模式规范化、Unicode 通配符和错误校验。
- `src/fetch/lsp.rs`、`src/fetch/treesitter.rs` 与 `src/tui/search.rs` 覆盖 provider、语言与搜索展示层的限定方法名。
- `src/app.rs` 同时验证搜索和 hierarchy 过滤，并确认大小写与整串匹配边界。

语言服务器协议不保证 call hierarchy 一定提供容器信息，且 `detail` 格式由 server 自定。cgraph 不跨语言猜测；缺少对应 adapter、信息缺失或内容是路径/签名时保留 server 原始名称。
