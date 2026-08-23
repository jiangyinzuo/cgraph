# REQ-5：符号与图入口管理

| 字段 | 值 |
| --- | --- |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 目标

用户能够搜索当前项目中的 call/type 符号，将结果作为 anchor 加入或重新定位到现有关系图，并取消不需要的入口或清除方向分支。

## 子需求

| 子需求 | 状态 | 摘要 |
| --- | --- | --- |
| [REQ-5-1 ac/at 搜索弹窗](REQ-5-1-search-modal.md) | `Implemented` | 打开、输入、选择和接受结果 |
| [REQ-5-2 异步查询生命周期](REQ-5-2-query-lifecycle.md) | `Implemented` | 防抖、取消和拒绝迟到结果 |
| [REQ-5-3 候选过滤与排序](REQ-5-3-filter-ranking.md) | `Implemented` | 类型过滤、工作区范围和模糊排名 |
| [REQ-5-4 新增或重定位图入口](REQ-5-4-root-placement.md) | `Implemented` | 复用语义节点并固定为 anchor |
| [REQ-5-5 取消入口和清除分支](REQ-5-5-delete.md) | `Implemented` | `dd`、`dp`、`dn` 的图语义 |

## 父需求验收

- 用户不离开 TUI 即可发现当前项目符号。
- 搜索输入保持响应，旧请求不能覆盖新结果。
- 接受结果后画布定位到正确语义符号。
- 用户可以取消显式图入口或清除指定方向分支，不误删共享节点。

## 当前实现与差距

workspace symbol 搜索、全图语义节点复用、anchor 创建/重定位、取消入口和共享安全的方向分支清除均已交付。层次关系的实际展开仍由 `REQ-3` 管理。
