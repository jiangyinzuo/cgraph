# 项目配置

cgraph 会读取 `--workspace` 根目录中的 `.cgraph.toml`。该文件适合提交到项目版本库，使团队成员看到一致的关系图；没有这个文件时仍使用内置 LSP 自动检测和项目范围过滤，符号名过滤规则保持为空。

## 语言服务器

可以在项目配置中直接指定 stdio LSP 命令和参数：

```toml
[lsp]
name = "clangd"
command = "clangd"
args = ["--background-index"]
file_extensions = ["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"]
```

`name` 指定内置 server profile/语言方言，`command` 表示实际可执行文件或路径；例如包装脚本可以使用 `name = "clangd"`。`args` 中每个元素作为一个独立参数传递，不进行 shell 解析。`file_extensions` 控制 profile 查找可用于启动索引的项目文档；每项只写不带路径和通配符的后缀。加载时会去掉可选前导 `.`、转成小写并去重。省略该字段时使用 profile 默认值：Rust 为 `rs`，clangd 为 `c/cc/cpp/cxx/h/hh/hpp/hxx`，Pyrefly 为 `py/pyi`；显式空数组是配置错误。自定义语言服务器应显式声明自己的后缀。省略 `[lsp]` 时，cgraph 使用内置默认 profile：Rust 为 `rust-analyzer`，C/C++ 为 `clangd`，Python 为 `pyrefly lsp`。CLI 的 `--lsp` 与 `--lsp-arg` 是一次性覆盖，优先于项目配置；`--no-lsp` 会完全禁用 LSP。通过 `ec` 修改命令后，过滤配置会在当前会话刷新，但新的 LSP 命令、参数和文件扩展名在下一次启动时生效。

## 过滤符号

```toml
[filters]
workspace_only = true
rules = ["#<all>", "!#main", "!#Cli::run", "**/generated/**", "!src/generated/keep.rs"]
```

规则按数组顺序从前往后应用，普通规则排除，`!` 规则重新包含，最后一个命中的规则获胜。普通规则按 workspace-relative 路径匹配；以 `#` 开头的规则按完整限定名匹配。`#<all>` 表示所有符号；符号 `*` 可匹配任意字符。路径 `*` 不跨目录，`**` 可跨目录，且不含 `/` 的模式会匹配任意目录中的文件名。

`filters.workspace_only` 默认是 `true`，会同时限制 workspace symbol 搜索和 LSP hierarchy 返回项：只有位于当前 workspace 根目录下的 `file://` URI 才会进入图。也可以在 `rules` 中使用 `<workspace>`（排除 workspace 外路径）和 `!<workspace>`（重新放行）表达同样的范围策略。若确实需要查看语言服务器返回的系统头文件或第三方依赖，可以设置：

```toml
[filters]
workspace_only = false
```

关闭范围过滤只表示 cgraph 保留这些外部节点；外部节点能否继续展开仍取决于语言服务器的文档生命周期要求。比如 clangd 可能要求客户端先打开系统头文件，展开 `printf` 仍可能返回 `trying to get AST for non-added document`。

如果只想保留当前 workspace，同时为一个外部符号开例外，可以利用跨类型规则的书写顺序：

```toml
[filters]
rules = ["<workspace>", "!#printf"]
```

第一条路径规则排除 workspace 外的所有候选；第二条更晚的符号规则只把完整名称为 `printf` 的候选重新包含。

符号规则（`#` 前缀）针对界面中的完整显示名，并且必须匹配整个名称；普通规则排除、`!` 规则重新包含：

- 普通函数使用 server 返回的名称，例如 `main`。
- 方法和构造器在对应 provider 能够确定类或容器时显示完整限定名；Rust/C++ 使用 `Class::method`，Python 使用 `Class.method`。
- Rust 没有 class 关键字；cgraph 使用方法所属的 struct、enum 或 trait 名。trait impl 使用具体实现类型，例如 `impl Read for Buffer` 显示为 `Buffer::read`。
- `*` 匹配零个或多个字符，包括 `::`。
- 匹配区分大小写，所以 `*::Some` 不会过滤 `Option::some`。
- 规则必须使用实际显示分隔符；`*::run` 不会匹配 Python 的 `Worker.run`，应写成 `*.run`。
- 没有通配符的规则是整串精确匹配；`Option::is_some` 不会过滤 `Result::is_some`。

过滤同时作用于 `ac` / `at` 搜索候选和 `tl` / `tr` 新加载的邻接节点。它不会删除已经显式写在命令行中的 anchor，例如 `cgraph call Option::is_some` 仍会创建该入口。

## 在 TUI 中编辑和重载

Canvas 模式输入 `ec` 会离开备用屏幕，并用 `$EDITER` 打开当前 workspace 的 `.cgraph.toml`。项目没有配置文件时，cgraph 先用 `create_new` 创建以下最小有效模板，不覆盖已经存在的文件：

```toml
# Optional language-server command.
# When omitted, cgraph selects rust-analyzer, clangd or pyrefly by project markers.
#[lsp]
# name = "rust-analyzer"
# command = "rust-analyzer"
# args = []
# file_extensions = ["rs"]

[filters]
# Keep discovered symbols inside the project root.
workspace_only = true
# Ordered rules; prefix symbol patterns with #.
rules = []
```

项目按需求使用变量名 `$EDITER`；如果没有设置，cgraph 兼容标准的 `$EDITOR`。变量值必须是可直接执行的程序或路径，例如：

```bash
export EDITER=nvim
cgraph --workspace .
```

编辑器成功退出后，cgraph 重新进入 raw mode 和备用屏幕，严格校验配置并替换当前过滤器。所有已经加载或正在刷新的可达 incoming/outgoing 分支都会收到新的 refresh request，包括成功查询为空的分支；这样收紧规则会移除关系，放宽规则也能恢复之前被过滤的关系。未加载分支仍保持惰性，anchors 不会因过滤器而被删除。

编辑器无法启动、非零退出或配置无效时，cgraph 恢复 TUI、保留上一份有效配置和当前图，并在倒数第二行显示原因，同时写入 `g<` 消息历史。没有分析 provider 时仍会重载规则，但无法重新查询缓存关系，消息行会明确说明这一点。

## 加载与错误

无效 TOML、未知字段、错误的数据类型和空字符串模式都会阻止启动，错误消息包含 `.cgraph.toml` 的路径。cgraph 不会静默忽略拼错的字段，因为那会让用户误以为过滤已经生效。

call hierarchy 协议不强制语言服务器提供类名，`detail` 也没有跨服务器统一格式。cgraph 按语言/provider 分别适配；Rust LSP adapter 会从 rust-analyzer 的 document symbols 查找 `impl` 容器并按文件缓存，Tree-sitter 则从 impl/class AST ancestor 读取容器。若 provider 没有返回可识别的容器信息，cgraph 会保留原始方法名，此时只能用该实际显示名编写规则。
