# REQ-2：分析后端状态

| 字段 | 值 |
| --- | --- |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 目标

用户能够在 TUI 中判断当前由哪个源码分析后端提供能力，以及它是就绪、工作、警告、失败还是断开。

## 子需求

| 子需求 | 状态 | 摘要 |
| --- | --- | --- |
| [REQ-2-1 LSP 状态与进度](REQ-2-1-lsp-progress.md) | `Implemented` | 标准 progress、rust-analyzer 状态和断开 |
| [REQ-2-2 Tree-sitter 状态](REQ-2-2-tree-sitter-status.md) | `Implemented` | 初始化、就绪与失败状态 |
| [REQ-2-3 底部状态栏布局](REQ-2-3-status-window.md) | `Implemented` | 快捷键与后端状态同栏展示 |

## 父需求验收

- 状态模型不依赖具体后端协议。
- LSP 连接与后台进度可以被用户观察。
- LSP 不可用且能够检测语言时，Tree-sitter 通过同一底栏报告初始化、就绪或失败。
- 全局后端状态与单次 workspace symbol 查询状态保持独立。
- 普通信息和错误统一进入消息历史，并在倒数第二行显示最新摘要；最底行快捷键与分析状态始终保留。
- 消息 pager 从倒数第二行向上最多显示 15 行，支持行、整页、半页、首尾滚动以及 `V` 选择、`y` OSC 52 复制，并用 `q` / `Esc` 返回画布。
- pager 打开时终端接管鼠标文本选择，关闭后恢复 Canvas 鼠标捕获；OSC 52 复制依赖终端能力。

## 当前实现与差距

LSP progress、统一底部状态栏和 Tree-sitter fallback 状态均已交付。Tree-sitter 会初始化对应 grammar 与 tags query，Ready 消息说明项目静态索引在第一次查询时建立；搜索 modal/分支 loading 表示单次工作，hierarchy 完成后的消息摘要明确说明语法级置信度。所有操作信息和错误进入统一历史，摘要位于倒数第二行；`g<` 打开纯 Ratatui pager，提供接近 `less` 的滚动、行选择和复制，同时保持最底行快捷键可见。
