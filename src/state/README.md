# State 层设计

相关产品规范：[REQ-3 层次关系探索](../../requirements/REQ-3-hierarchy/README.md)、[REQ-4 画布与导航](../../requirements/REQ-4-canvas-navigation/README.md)、[REQ-5 符号与树管理](../../requirements/REQ-5-symbol-management/README.md)。State 层保存这些需求需要的不变量，但不决定按键和视觉样式。

State 层保存与 UI 框架、LSP 协议无关的领域状态。这里的类型应能被 TUI、查询层和 IPC 共同使用。

## 身份迁移

当前树实现区分“画布节点实例”和“语义符号”：

- `NodeId`：每次把符号放进树时生成，用于选择、布局和局部展开。按照产品要求，同一个符号可以在树中出现多次，所以每个实例都有不同的 `NodeId`。
- `SymbolIdentity`：由 hierarchy kind、名称和可选源码位置组成，用于查询缓存、刷新匹配以及同步高亮相同符号。

有向图迁移后，同一 resolved 语义符号只保留一个画布节点，但 `NodeId` 与符号身份仍然不能混用。不能仅以名称判定语义相同，因为重载、同名方法和不同文件中的类型都很常见。缺少源码位置的 CLI 查询属于 provisional identity，后续通过 workspace symbol 或 prepare hierarchy 补全并显式合并。完整决策见[有向关系图领域模型](graph-model.md)。

## 当前树模型与迁移目标

每个 `Node` 有两个 `Branch`：

- `incoming`：call hierarchy 中的 callers，type hierarchy 中的 supertypes。
- `outgoing`：call hierarchy 中的 callees，type hierarchy 中的 subtypes。

分支同时保存 `expanded`、`load_state` 和 `children`。`expanded = false` 不代表没有缓存；它只表示当前不渲染孩子。再次展开时应恢复此前孩子各自的展开深度。

左右分支必须独立 toggle。键盘 `tl` 对应 incoming，`tr` 对应 outgoing；`NotLoaded` 或 `Failed` 的空分支会生成带 request id 的懒加载请求，`Loading` 不生成重复请求，`Loaded` 则只切换可见性。toggle 只改变当前节点的目标分支，不递归改写孩子自身的展开状态。

`active_request_id` 是分支级并发防线。删除分支、删除树或失败后重试都会让旧结果失去匹配 id，因此迟到响应不能覆盖更新后的状态。成功空结果是 `Loaded` 且没有孩子，和 `Failed` 必须保持可观察区别。

App 当前在写入每个分支缓存前按完整 `SymbolIdentity` 去重，并保留首次出现顺序；不同父节点或不同路径不会全局去重。迁移目标以 `StableDiGraph` 保存唯一语义节点和规范边，分支只保存展开/加载状态、有序 neighbor 和对边的 observation。可见图从 anchors 沿展开分支计算，不能继续递归拥有孩子节点。

## 刷新不变量

刷新节点时，后续实现必须满足：

1. 只重新查询目标节点左右各一层，不递归刷新。
2. 用 `SymbolIdentity` 匹配新旧孩子，而不是用 `NodeId`。
3. 仍存在的孩子保留原 `NodeId`、分支缓存和展开状态。
4. 已消失的孩子及其子树从所有该语义节点实例中删除。
5. 新出现的孩子创建为未加载、未展开状态。

图模型中每个语义节点只有一份方向缓存。刷新需要按 branch observation 更新共享边，并保留仍然存在的节点、边和加载状态。

## 画布视口状态

`App` 拥有一个 `Viewport`，只保存世界坐标到终端坐标的有符号平移量。鼠标拖拽通过 `App::pan_viewport` 以饱和加法累积增量，不修改 `Node`、`NodeId` 或分支结构；接受新的或已有的搜索结果时重置 viewport，使目标根回到画布中心。

世界矩形、终端 `Rect`、拖拽锚点和裁剪策略都属于 TUI 视图细节，不进入 State。这样 terminal resize 只会重新投影已有领域状态，IPC 和无头测试也不需要依赖 Ratatui。

后续建议增加：

- `RelationGraph`：拥有有向图、anchors、语义身份索引和容器索引映射。
- `LayoutSnapshot`：缓存一次世界布局计算的不可变结果，避免大型树每帧重复遍历。
- `SelectionState`：当前实例和所有同语义实例集合。
- `RefreshPlan`：以纯数据描述保留、删除和新增哪些孩子，便于单元测试。
