# 有向关系图领域模型决策

## 状态

Accepted，2026-08-23。对应 `REQ-3-3`、`REQ-4-3`、`REQ-5-4` 和 `REQ-5-5`。

## 背景

早期实现以递归 `Node` 树保存 hierarchy。`Branch.children` 同时承担邻接关系、孩子所有权、展开状态和查询缓存，因此同一语义符号沿不同路径出现时会创建多个画布实例。该模型容易表达局部展开，但不能自然表示菱形汇合、循环、自环或多个搜索入口共享的关系。

产品现在要求画布显示有向图：相同语义节点全局去重，同时保留所有关系边。call 的规范边为 caller 到 callee，type 的规范边为 parent/supertype 到 child/subtype。正常边应尽量从左向右，无法满足该方向的边必须明确标记。

## 决策

领域层采用 `petgraph::stable_graph::StableDiGraph<NodeData, EdgeData>` 保存关系，并通过项目自己的 `NodeId` 隔离容器索引。App 和异步请求不得暴露 `NodeIndex` 或 `EdgeIndex`。

选择 `StableDiGraph` 的原因：

- hierarchy 查询和刷新会增量增加、合并或移除节点与边；
- 异步结果可能晚于删除或重试到达，其他节点的内部索引不能因一次删除整体移动；
- petgraph 提供环安全遍历、强连通分量和拓扑算法，可用于可见图与分层布局；
- 节点有复杂可变 payload，不适合要求轻量节点键的 `GraphMap`。

依赖只启用 `std` 和 `stable_graph`，避免引入当前不使用的 `graphmap`、`matrix_graph` 等默认 feature。`petgraph` 不负责终端布局和路由；这些仍属于 `tui/canvas`。

## 身份与索引

`NodeId` 是进程内稳定句柄，用于选择、请求关联和布局快照。resolved identity 使用 hierarchy kind、规范化 URI、行和列；展示名称是节点属性，不作为精确位置符号的唯一键。

CLI 可能创建没有源码位置的入口。这类节点使用 provisional identity，不能仅凭名称与其他节点合并。prepare hierarchy 补全位置后执行显式 resolve/merge：重定向 anchors、选择、边和分支引用，再更新语义索引。

## 节点、分支和边

唯一节点拥有 incoming/outgoing 两个 `ExpansionState`。每个方向独立保存 `expanded`、`load_state`、有序 neighbor 列表、错误和 active request id。收起只改变 `expanded`，不会清除 neighbor 缓存或孩子节点自身状态。

边使用规范 source/target。incoming 查询当前节点 N 时，返回节点 P 产生 `P -> N`；outgoing 查询返回 C 时产生 `N -> C`。同一条边可能由两个端点的方向查询发现，`EdgeData` 记录 observation owner。清除或刷新一个分支只撤销自己的 observation，最后一个 observation 消失后才能移除语义边。

## Anchor 与可见图

搜索或 CLI 创建的是 anchor，而不是拥有子树的 root。可见图从所有 anchors 开始，按当前可见节点的 expanded incoming/outgoing neighbor 进行 visited-set 遍历。缓存中存在但不能从 anchor 经展开分支抵达的节点不渲染；多个 anchor 的结果取并集。

`dd` 取消当前 anchor，不删除连通分量。普通节点执行 `dd` 不产生隐式破坏。`dp` / `dn` 清除方向分支及其 edge observations。未被 anchor 抵达的数据可以继续作为进程内语义缓存保留，后续搜索可复用。

## 布局边界

领域层只生成不含 Ratatui 类型的可见节点和规范边。TUI 对可见图计算 SCC，把收缩 DAG 从左向右分层，再生成包含节点矩形、按钮和路由后的 `LayoutSnapshot`。目标不在源右侧的边、自环和 SCC 内部边使用反向/循环视觉样式。

## 后果

- 递归 `Node::find/find_mut/contains` 将被图索引查询替代。
- 原先“删除节点所在树”不再有唯一含义，需求改为取消 anchor。
- 同一语义节点只有一份展开和加载状态；不同路径不再拥有互相冲突的节点实例状态。
- 图版本变化时才需要重新构建可见图和布局快照，避免每个终端帧重复执行 SCC。
- 导出和 IPC 未来应使用项目级 `NodeId`/语义身份，不持久化 petgraph 索引。
