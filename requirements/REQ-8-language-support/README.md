# REQ-8：语言支持

| 字段 | 值 |
| --- | --- |
| 状态 | `Implemented` |
| 优先级 | `P1` |
| 目标版本 | `0.1` |
| 子需求 | 1 |

## 目标

cgraph 首先支持 Rust、C/C++ 和 Python 项目，并允许用户显式选择兼容的 stdio LSP server。Python 项目默认使用 Pyrefly。

## 子需求

| 子需求 | 状态 | 摘要 |
| --- | --- | --- |
| [REQ-8-1 Pyrefly 默认 Python LSP](REQ-8-1-pyrefly.md) | `Implemented` | 自动选择 `pyrefly lsp`、标准 hierarchy 和 Python 限定名 |

## 需求

- Rust 工作区默认检测 `rust-analyzer`。
- C/C++ 工作区的源文件或头文件默认检测 `clangd`。
- Python 工作区默认检测 Pyrefly，并以 `pyrefly lsp` 启动。
- `.cgraph.toml` 可以配置 LSP name/command/args 和项目文件后缀；显式 `--lsp` 和重复 `--lsp-arg` 可以作为一次性覆盖。
- 后端能力不足时显式降级或报错，不假设所有 server 都实现相同 hierarchy 能力。
- 没有可用 LSP 时，使用项目本地 Tree-sitter 静态索引提供 workspace symbol 和可判定的 hierarchy 关系；LSP 只缺少某种 hierarchy 能力时允许按查询粒度回退。

## 验收条件

- 三类项目标志可以触发浅层、可预测的自动检测；C/C++ 检测覆盖常见头文件，`pyrefly.toml` 也能直接标识 Python 工作区。
- workspace symbol 搜索可以通过标准 LSP 接口工作；真实 Pyrefly、rust-analyzer、clangd 条件测试覆盖 call hierarchy，并验证 type hierarchy 或同一生产回退链路。
- call/type hierarchy 支持情况按 server capability 判断。
- 自动检测不递归猜测复杂 monorepo；用户可以显式覆盖。
- Tree-sitter 回退必须覆盖四种语言的项目符号；Rust/Python/C/C++ 覆盖直接静态调用，Rust/C++/Python 覆盖各自存在的项目内类型继承语法。
- Tree-sitter 的语法级结果必须明确标记，不把动态或歧义关系的未知状态伪装成完整 LSP 语义。

## 当前实现与差距

rust-analyzer、clangd 和 Pyrefly 的内置 profile、项目配置/CLI 命令组装、通用 workspace symbol 搜索以及标准 LSP call/type hierarchy 客户端已经实现。Pyrefly 的 `lsp` 子命令由 cgraph 自动补充，方法名使用 `Class.method`；用户仍可用项目 `[lsp]` 配置、`--lsp pylsp` 或其他程序覆盖默认选择。call hierarchy 的 initialize capability 与 call/type 动态 registration 会被实际追踪，未声明的请求不会发送。当前 rust-analyzer 缺少标准 type hierarchy，因此 Rust 类型查询自动使用 Tree-sitter trait-impl 关系，搜索和 call hierarchy 仍使用 rust-analyzer。

Tree-sitter provider 递归索引项目源文件，跳过隐藏目录、`target`、`node_modules` 和符号链接；四种语言共享 workspace-symbol 与 hierarchy client 接口。Rust 方法使用 `Type::method`，C++ 使用 `Class::method`，Python 使用 `Class.method`。静态索引只绑定唯一的项目内目标，成功结果通过倒数第二行消息标明语法级置信度并进入历史。单语言会话可以同时持有一个 LSP 主后端和同语言 Tree-sitter hierarchy 后备，但不会聚合两者的同一次查询结果；复杂多语言 monorepo 的多 provider 聚合仍属于后续增强。
