# REQ-1：会话与启动

| 字段 | 值 |
| --- | --- |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 目标

用户能够从当前工作区启动 cgraph，选择空画布或指定初始 call/type anchor，并可靠退出 TUI。

## 子需求

| 子需求 | 状态 | 摘要 |
| --- | --- | --- |
| [REQ-1-1 启动与初始图入口](REQ-1-1-startup.md) | `Implemented` | 空画布、call anchor 和 type anchor 三种入口 |
| [REQ-1-2 退出与终端恢复](REQ-1-2-exit.md) | `Implemented` | `q` / `Esc` 退出并恢复终端 |

## 父需求验收

- `cgraph` 能打开空画布。
- `cgraph call <SYMBOL>` 和 `cgraph type <SYMBOL>` 能显示对应 anchor。
- 用户能正常退出，终端 raw mode、备用屏幕和鼠标捕获被恢复。

## 当前边界

命令行名称创建的 anchor 仍可能缺少精确源码位置；名称解析属于 `REQ-3` 的 hierarchy prepare 前置工作，不影响本需求的启动入口已经交付。
