# REQ-8：语言支持

| 字段 | 值 |
| --- | --- |
| 状态 | `Implemented` |
| 优先级 | `P1` |
| 目标版本 | `0.1` |
| 子需求 | 无 |

## 目标

cgraph 首先支持 Rust、C/C++ 和 Python 项目，并允许用户显式选择兼容的 stdio LSP server。

## 需求

- Rust 工作区默认检测 `rust-analyzer`。
- C/C++ 工作区默认检测 `clangd`。
- Python 工作区默认检测 `pylsp`。
- 显式 `--lsp` 和重复 `--lsp-arg` 可以覆盖自动检测。
- 后端能力不足时显式降级或报错，不假设所有 server 都实现相同 hierarchy 能力。
- 没有可用 LSP 时，使用项目本地 Tree-sitter 静态索引提供 workspace symbol 和可判定的 hierarchy 关系。

## 验收条件

- 三类项目标志可以触发浅层、可预测的自动检测。
- workspace symbol 搜索可以通过标准 LSP 接口工作。
- call/type hierarchy 支持情况按 server capability 判断。
- 自动检测不递归猜测复杂 monorepo；用户可以显式覆盖。
- Tree-sitter 回退必须覆盖四种语言的项目符号；Rust/Python/C/C++ 覆盖直接静态调用，Rust/C++/Python 覆盖各自存在的项目内类型继承语法。
- Tree-sitter 的语法级结果必须明确标记，不把动态或歧义关系的未知状态伪装成完整 LSP 语义。

## 当前实现与差距

三种 server 的启动检测、通用 workspace symbol 搜索以及标准 LSP call/type hierarchy 客户端已经实现。各 server 是否实际支持对应标准方法会以成功、空结果或明确错误呈现。

没有可用 LSP 时，Tree-sitter provider 递归索引项目源文件，跳过隐藏目录、`target`、`node_modules` 和符号链接；四种语言共享 workspace-symbol 与 hierarchy client 接口。Rust 方法使用 `Type::method`，C++ 使用 `Class::method`，Python 使用 `Class.method`。静态索引只绑定唯一的项目内目标，成功结果通过 footer notice 标明语法级置信度。当前会话仍只选择一个分析后端；复杂多语言 monorepo 的多 provider 聚合属于后续增强，不阻塞本需求验收。
