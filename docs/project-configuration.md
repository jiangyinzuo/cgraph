# 项目配置

cgraph 会读取 `--workspace` 根目录中的 `.cgraph.toml`。该文件适合提交到项目版本库，使团队成员看到一致的关系图；没有这个文件时仍默认只展示项目内符号，符号名过滤规则保持为空。

## 过滤符号

```toml
[filters]
workspace_only = true
symbols = [
  "*::into",
  "Option::is_some",
  "*::Some",
  "*.run",
]
```

`filters.workspace_only` 默认是 `true`，会同时限制 workspace symbol 搜索和 LSP hierarchy 返回项：只有位于当前 workspace 根目录下的 `file://` URI 才会进入图。若确实需要查看语言服务器返回的系统头文件或第三方依赖，可以设置：

```toml
[filters]
workspace_only = false
```

关闭范围过滤只表示 cgraph 保留这些外部节点；外部节点能否继续展开仍取决于语言服务器的文档生命周期要求。比如 clangd 可能要求客户端先打开系统头文件，展开 `printf` 仍可能返回 `trying to get AST for non-added document`。

规则针对界面中的完整显示名，并且必须匹配整个名称：

- 普通函数使用 server 返回的名称，例如 `main`。
- 方法和构造器在对应 provider 能够确定类或容器时显示完整限定名；Rust/C++ 使用 `Class::method`，Python 使用 `Class.method`。
- Rust 没有 class 关键字；cgraph 使用方法所属的 struct、enum 或 trait 名。trait impl 使用具体实现类型，例如 `impl Read for Buffer` 显示为 `Buffer::read`。
- `*` 匹配零个或多个字符，包括 `::`；这是当前唯一的通配符。
- 匹配区分大小写，所以 `*::Some` 不会过滤 `Option::some`。
- 规则必须使用实际显示分隔符；`*::run` 不会匹配 Python 的 `Worker.run`，应写成 `*.run`。
- 没有通配符的规则是整串精确匹配；`Option::is_some` 不会过滤 `Result::is_some`。

过滤同时作用于 `ac` / `at` 搜索候选和 `tl` / `tr` 新加载的邻接节点。它不会删除已经显式写在命令行中的 anchor，例如 `cgraph call Option::is_some` 仍会创建该入口。

## 在 TUI 中编辑和重载

Canvas 模式输入 `ec` 会离开备用屏幕，并用 `$EDITER` 打开当前 workspace 的 `.cgraph.toml`。项目没有配置文件时，cgraph 先用 `create_new` 创建以下最小有效模板，不覆盖已经存在的文件：

```toml
[filters]
# Keep discovered symbols inside the project root.
workspace_only = true
# Full symbol names; * matches any number of characters.
symbols = []
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
