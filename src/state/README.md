# State 层设计

相关产品规范：[REQ-3 层次关系探索](../../requirements/REQ-3-hierarchy/README.md)、[REQ-4 画布与导航](../../requirements/REQ-4-canvas-navigation/README.md)、[REQ-5 符号与图入口管理](../../requirements/REQ-5-symbol-management/README.md)。State 层保存这些需求需要的不变量，但不决定按键和视觉样式。

State 层保存与 UI 框架、LSP 协议无关的领域状态。这里的类型应能被 TUI、查询层和 IPC 共同使用。

## 节点身份

图模型区分进程内节点句柄与语义符号身份：

- `NodeId`：进程内稳定句柄，用于选择、异步请求、anchor 和布局快照。它不等于 petgraph `NodeIndex`，也不作为持久化身份。
- `SymbolIdentity`：provider 归一化结果，包含 hierarchy kind、展示名称和可选源码位置。
- resolved key：有位置时使用 hierarchy kind、URI、行和列建立全图索引；展示名称不参与精确位置节点的唯一性判断。
- provisional identity：CLI 名称尚未解析位置时使用独立 `NodeId`，prepare 补全后通过 redirect 与已有 resolved 节点显式合并。

`SourceLocation` 的 line 和 character 都是可选的零基 LSP 坐标，character 统一按 UTF-16 code units 解释。LSP provider 只协商 UTF-16，Tree-sitter provider 在写入 State 前完成字节列转换；State 不保存 provider 专用编码。只有 URI、line 和 character 都存在的位置才能用于编辑器跳转。

外部 `focus_symbol` 经过 App 使用同一身份规则：有精确位置时直接 `pin_symbol`，因此本地搜索与 IPC 不会为同一 resolved key 创建两个节点；没有位置时按 hierarchy kind 和完整展示名查找，唯一候选复用、零候选创建 provisional anchor、多候选返回 ambiguous。无位置名称不能作为全图唯一 key，否则不同文件的同名函数会被错误合并。

同一 resolved 语义符号只保留一个画布节点，但 `NodeId` 与符号身份仍然不能混用。不能仅以名称判定语义相同，因为重载、同名方法和不同文件中的类型都很常见。完整决策见[有向关系图领域模型](graph-model.md)。

## 关系图、分支和可见性

`RelationGraph` 使用 `StableDiGraph<GraphNode, RelationEdge>` 保存全局唯一节点与规范边。每个 `GraphNode` 有两个 `GraphBranch`：

- `incoming`：call hierarchy 中的 callers，type hierarchy 中的 supertypes。
- `outgoing`：call hierarchy 中的 callees，type hierarchy 中的 subtypes。

分支保存 `expanded`、`load_state`、有序 `neighbors`、错误和 active request id。`expanded = false` 不代表没有缓存；它只表示当前不沿该方向扩展可见图。再次展开时会恢复邻接节点各自已有的深层展开状态。

左右分支必须独立 toggle。键盘 `tl` 对应 incoming，`tr` 对应 outgoing；`NotLoaded` 或 `Failed` 的空分支会生成带 request id 的懒加载请求，`Loading` 不生成重复请求，`Loaded` 则只切换可见性。toggle 只改变当前节点的目标分支，不递归改写孩子自身的展开状态。

`active_request_id` 是分支级并发防线。清除分支、节点合并或失败后重试都会让旧结果失去匹配 id，因此迟到响应不能覆盖更新后的状态。成功空结果是 `Loaded` 且没有 neighbors，和 `Failed` 必须保持可观察区别。

call 的规范边为 caller 到 callee，type 的规范边为 parent/supertype 到 child/subtype。incoming/outgoing 查询可能发现同一规范边；`RelationEdge.observed_by` 记录所有 branch owner，清除一个方向不会删除仍被其他查询确认的边。

`visible_graph` 从所有 anchors 开始，只沿当前可见节点的 expanded neighbors 进行 visited-set 遍历。这样缓存数据与显示状态分离，祖先收起会隐藏不可达后代，循环和自环不会无限递归，多个路径到达同一节点时只显示一次。

`known_graph` 使用同样的 anchor 可达性和去重规则，但遍历已经缓存的全部 neighbors，不检查 expanded。文本导出因此不会因临时收起分支而丢失数据，同时也不会泄露取消 anchor 或清除关系后留在内部存储、但已经不可达的孤立节点。

## 刷新不变量

刷新节点时，当前实现满足：

1. 只重新查询目标节点左右各一层，不递归刷新。
2. 用 `SymbolIdentity` 匹配新旧孩子，而不是用 `NodeId`。
3. 仍存在的 neighbor 保留原 `NodeId`、分支缓存和展开状态。
4. 已消失的关系只撤销目标 branch observation；其他来源仍确认的边保留。
5. 新出现的 neighbor 创建为未加载、未展开状态。

图模型中每个语义节点只有一份方向缓存。App 的 `CachePolicy::Refresh` 请求同时覆盖左右方向；成功结果通过 `replace_branch_neighbors` 按 branch observation 更新共享边，并保留仍然存在的节点、边和加载状态。刷新开始只把分支标为 `Loading`，不清空 neighbors 或改变 expanded；失败恢复请求前的稳定 load state，并保留错误供 footer 诊断。连续刷新用新的 active request id 使旧结果失效。

## 画布视口状态

`App` 拥有一个 `Viewport`，只保存世界坐标到终端坐标的有符号平移量。鼠标拖拽通过 `App::pan_viewport` 以饱和加法累积增量，不修改 `GraphNode`、`NodeId` 或边；TUI 在选择、toggle 和异步 hierarchy 完成前后用同一方法补偿目标节点的世界中心差值，使其屏幕位置稳定。接受新的或已有的搜索结果时仍会显式重置 viewport，使目标 anchor 回到画布中心。

世界矩形、终端 `Rect`、拖拽锚点和裁剪策略都属于 TUI 视图细节，不进入 State。这样 terminal resize 只会重新投影已有领域状态，IPC 和无头测试也不需要依赖 Ratatui。

后续建议增加：

- 图版本号与持久化 `LayoutSnapshot` 缓存，避免无状态变化时每帧重复运行 SCC。
