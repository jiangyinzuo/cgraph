# REQ-8：语言支持

| 字段 | 值 |
| --- | --- |
| 状态 | `Partial` |
| 优先级 | `P1` |
| 目标版本 | `TBD` |
| 子需求 | 无 |

## 目标

ctree 首先支持 Rust、C/C++ 和 Python 项目，并允许用户显式选择兼容的 stdio LSP server。

## 需求

- Rust 工作区默认检测 `rust-analyzer`。
- C/C++ 工作区默认检测 `clangd`。
- Python 工作区默认检测 `pylsp`。
- 显式 `--lsp` 和重复 `--lsp-arg` 可以覆盖自动检测。
- 后端能力不足时显式降级或报错，不假设所有 server 都实现相同 hierarchy 能力。

## 验收条件

- 三类项目标志可以触发浅层、可预测的自动检测。
- workspace symbol 搜索可以通过标准 LSP 接口工作。
- call/type hierarchy 支持情况按 server capability 判断。
- 自动检测不递归猜测复杂 monorepo；用户可以显式覆盖。
- Tree-sitter 回退启用前，不把它计入任何语言的已交付支持。

## 当前实现与差距

三种 server 的启动检测、通用 workspace symbol 搜索以及标准 LSP call/type hierarchy 客户端已经实现。各 server 是否实际支持对应标准方法会以成功、空结果或明确错误呈现；Tree-sitter hierarchy 尚未实现，且当前会话只能连接一个 server，因此父需求仍为 `Partial`。
