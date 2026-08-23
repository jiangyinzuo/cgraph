# REQ-3-3：全局语义节点去重

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-3` |
| 状态 | `Implemented` |
| 优先级 | `P1` |
| 目标版本 | `TBD` |

## 需求

画布使用有向图表示 call/type hierarchy。同一 hierarchy kind 和精确源码位置标识的语义符号只显示为一个节点；不同路径、不同查询方向和多个入口发现的关系都连接到该共享节点，不能因节点去重而丢失边。

有精确位置的符号以 hierarchy kind、规范化 URI、行和列作为稳定身份，展示名称不是唯一键。缺少位置的 CLI 查询使用临时身份，待 LSP prepare 补全后再与已有节点显式合并；不能只按名称合并重载或不同文件中的同名符号。

规范边方向始终从左向右：call hierarchy 为 caller 到 callee，type hierarchy 为 parent/supertype 到 child/subtype。incoming 和 outgoing 查询可能发现同一条规范边，图中只保留一条边，但必须记录所有仍然有效的查询来源。

## 验收条件

- 菱形关系的汇合符号只产生一个节点，所有入边和出边都保留。
- 循环和自环不会无限递归，也不会因为 visited 去重而丢失关系边。
- 同一条关系被两个查询方向发现时只渲染一次；清除一个查询来源不会删除仍被其他来源确认的边。
- 内部 `NodeId` 与可补全的符号身份分离，异步请求不暴露图容器索引。
- 同名但不同文件、位置或重载的符号不被错误合并。
- 收起分支只改变可见子图，不清除已经成功加载的邻接缓存。

## 当前实现与差距

`RelationGraph` 使用 `StableDiGraph`、resolved identity 索引和 provisional redirect 保存唯一节点；方向分支保存有序 neighbors，规范边记录 observation owner。可见图从 anchors 沿展开分支进行 visited-set 遍历，菱形、循环和自环不会复制节点或无限递归。

领域层测试覆盖菱形全局去重、双方向发现同一边、循环、自环、同名不同位置、provisional 合并和共享边清除；TUI 测试覆盖菱形单矩形及循环/自环特殊线条。

## 关联文档

- [图领域模型技术决策](../../src/state/graph-model.md)
- [REQ-4-3 有向图布局与反向边](../REQ-4-canvas-navigation/REQ-4-3-graph-layout.md)
