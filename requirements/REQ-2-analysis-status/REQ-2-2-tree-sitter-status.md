# REQ-2-2：Tree-sitter 状态

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-2` |
| 状态 | `Implemented` |
| 优先级 | `P2` |
| 目标版本 | `0.1` |

## 需求

Tree-sitter provider 启用后，应通过通用分析状态模型显示语言、初始化、工作、就绪和失败状态。

## 验收条件

- 底部状态栏显示 `Tree-sitter: <language>`。
- grammar/query 初始化失败显示 Error 和原因。
- provider 未启用时显示 inactive，不把“尚未实现”描述为自动回退成功。

## 当前实现与差距

当 LSP 不可用时，ctree 会浅层检测 Rust、C、C++ 或 Python 工作区，初始化对应 parser grammar 和 tags query，并通过统一状态模型报告 Working、Ready 或 Error。provider 保留可用 parser；workspace symbol 和 hierarchy 的 Tree-sitter 查询仍属于其他需求，不会被本状态伪装成已经支持。

## 实现证据

- 语言检测、四种 grammar/query 初始化和解析测试位于 `src/fetch/treesitter.rs`。
- fallback 选择和状态映射位于 `src/main.rs`。
