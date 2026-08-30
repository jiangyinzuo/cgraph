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
- 检测和选择后备期间可以短暂显示 Inactive；确认 LSP 与 Tree-sitter 都不可用后显示最终 Error，不把“尚未实现”描述为自动回退成功。

## 当前实现与差距

当 LSP 不可用时，cgraph 会浅层检测 Rust、C、C++ 或 Python 工作区，初始化对应 parser grammar 和 tags query，并通过统一状态模型报告 Working、Ready 或 Error。LSP 启动失败但 Tree-sitter 成功时，失败详情进入消息历史而不保留为当前 ERROR；确认两种 provider 都不可用时才报告最终错误。Ready 消息说明静态索引会在第一次搜索或展开时惰性建立；搜索弹窗继续显示单次索引/查询状态。LSP 正常时，同语言 Tree-sitter 可以只作为未声明 hierarchy kind 的惰性后备，此时底栏仍显示 LSP 主连接状态。任何 Tree-sitter hierarchy 结果都会在倒数第二行明确标为 `syntactic relations only` 并进入消息历史，与完整 LSP 语义区分。

## 实现证据

- 语言检测、四种 grammar/query 初始化和解析测试位于 `src/fetch/treesitter.rs`。
- fallback 选择和状态映射位于 `src/main.rs`。
