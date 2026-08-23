# REQ-5-5：取消入口和清除分支

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-5` |
| 状态 | `Implemented` |
| 优先级 | `P2` |
| 目标版本 | `0.1` |

## 需求

图模型中共享节点不属于唯一一棵树，因此删除操作按 anchor 和查询分支定义：

- `dd` 仅在当前节点是 anchor 时取消该 anchor；普通共享节点不执行隐式组件删除，并给出可见提示；
- `dp` 清除当前节点 incoming/parent 分支的缓存和该分支对边的确认；
- `dn` 清除当前节点 outgoing/child 分支的缓存和该分支对边的确认。

## 验收条件

- 前缀输入未完成时不会误删。
- 清除分支只影响唯一语义节点的目标方向，不删除其他分支仍然确认的边。
- 取消 anchor 后从剩余 anchors 重新计算可见图；没有 anchor 时显示空画布。
- `dd` 不按连通分量删除共享节点，也不清除仍可复用的语义缓存。
- 当前选择因操作不可见时，选择移动到确定的剩余 anchor；没有 anchor 时清空选择。
- 删除或清除产生的迟到异步结果不会恢复已经失效的分支状态。

## 实现证据

- RelationGraph 在 `src/state/graph.rs` 管理 anchor、edge observation 和分支清除。
- App 的 `delete_selected_anchor` 只取消当前 anchor；普通节点返回失败并显示 `Selected node is not an anchor`。
- TUI 的 `d` 前缀保持 `dd` / `dp` / `dn`，footer 使用图语义提示。
