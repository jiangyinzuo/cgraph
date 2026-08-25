# TUI 层设计

TUI 层把终端事件转换为 App 状态迁移，并把 App 渲染为 Ratatui widgets。业务不变量应尽量留在 App/State，TUI 只处理输入语义、坐标映射和视觉表现。

代码按变化原因拆分：`mod.rs` 保存事件循环、画布键鼠控制器和顶层渲染编排；`search.rs` 集中搜索防抖查询、输入、鼠标映射与弹窗渲染；`save.rs` 负责保存弹窗；`help.rs` 维护完整帮助内容、滚动和渲染；`config_editor.rs` 选择并启动外部编辑器；`canvas.rs` 负责世界布局、viewport 投影与完整矩形碰撞，其 `canvas/connections.rs` 子模块负责连线路由和字形。这样搜索、帮助、编辑器生命周期、节点布局和保存错误可以独立变化。

相关产品规范：[REQ-2 分析后端状态](../../requirements/REQ-2-analysis-status/README.md)、[REQ-4 画布与导航](../../requirements/REQ-4-canvas-navigation/README.md)、[REQ-5 符号与图入口管理](../../requirements/REQ-5-symbol-management/README.md)、[REQ-6 进程间通信](../../requirements/REQ-6-ipc/README.md)、[REQ-9 项目配置](../../requirements/REQ-9-project-configuration/README.md)。本文件解释实现理由，不替代这些需求的验收条件。

## 当前模式

事件处理分为两个模式：

- Canvas 模式：`a` 后接 `c` / `t` 打开 call/type 搜索框；`t` 后接 `l` / `r` 独立 toggle；`e` 后接 `c` 编辑并重载配置；裸 `r` 刷新当前节点；`w` 保存；`?` 打开帮助；`dd` / `dp` / `dn` 管理入口和分支；`q` 或 `Esc` 退出。
- Search modal 模式：普通字符编辑查询，方向键或 `Ctrl-n` / `Ctrl-p` 选择，回车确认，`Esc` 关闭。
- Save modal 模式：普通字符和 Backspace 编辑路径，回车尝试安全创建目标，`Esc` 关闭；写入失败保留弹窗和错误，成功后关闭并把路径写入统一消息历史。
- Help modal 模式：方向键、`j`/`k`、PageUp/PageDown、Home/End 或鼠标滚轮查看完整清单；`?`、`q`、`Esc` 只关闭帮助。帮助事件优先于 Canvas 分发，防止低层命令透传。
- Message pager 模式：最新信息或错误显示在倒数第二行；按 `g<` 从该行向上打开最多 15 行的 pager，支持滚动、`V` 行选择、`y` OSC 52 复制，`q`/`Esc` 关闭，最底行 footer 始终保留。

鼠标移动用于更新高亮，左键确认，滚轮移动选择。鼠标命中必须使用与渲染相同的 layout 函数，否则终端 resize 后视觉位置和点击位置会不一致。

## 底部分析状态栏

TUI 从 `main` 接收一个可选的 LSP status receiver，并在每轮事件循环绘制前 drain 已到达的通知。通知先映射为 App 的通用 `AnalysisStatus`，渲染代码只认识 LSP、Tree-sitter 或 none 三类后端及统一 phase，不直接解释 JSON-RPC。

```text
LSP $/progress / experimental/serverStatus
                  -> LspStatusUpdate channel
                  -> AnalysisStatus in App
                  -> footer right-side status summary
```

分析状态和搜索状态必须分开：`AnalysisStatus::Working` 表示 server 报告的全局后台任务，`SearchStatus::Loading` 只表示当前 `workspace/symbol` 已发送。将两者合并会在索引期间隐藏搜索完成状态，也会把一次慢查询误写成整个连接不可用。

footer 固定为最底部一行，快捷键提示后紧接分隔符和 `backend · phase [percentage] · message` 状态摘要。它们使用同一个 `Paragraph` 和完整底栏宽度，避免在小终端中为状态固定预留比例空白；终端不足时由统一行自然裁剪。LSP、Tree-sitter、phase 和状态详情使用不同颜色，但文本本身必须足以表达状态，不能只依赖颜色。

默认左侧只显示 `?`、添加、展开、移动和退出等高频入口。完整命令由帮助层维护；前缀等待时 footer 临时显示合法后缀。任何消息都不能替换该行：App 的保存、配置、IPC、查询结果和分析错误统一通过 `set_canvas_notice` / `set_canvas_error` 写入历史，并在独立的倒数第二行显示最新摘要。帮助清单和生产键位在同一 TUI 模块维护，新增快捷键时必须同步其测试与用户文档。

## Message pager

消息 pager 位于 `messages.rs`。普通信息、后端状态错误、workspace symbol 错误和 hierarchy 错误都先由 App 记录到统一历史；pager 只维护原始文本、垂直 offset、换行后的总行数、viewport 高度和可选行选择，不包含插入模式、编辑历史、寄存器或宏录制状态。渲染直接组合 Ratatui `Paragraph`、`Scrollbar`、`Block` 和 `Clear`，因此不会修改消息历史或工作区文件，也不依赖完整文本编辑器组件。

文本按 Unicode 显示宽度硬换行，offset 对换行后的屏幕行生效；打开时定位到最新消息，用户向上浏览后保持当前位置，回到底部后继续跟随新增消息。`j/k` 与方向键移动一行，`Space/f/b/PageUp/PageDown` 移动一页，`Ctrl-d/u` 移动半页，`g/G/Home/End` 跳转首尾。`V` 从当前活动屏幕行开始或取消 line selection，移动键扩展选择，`y` 按原始 source byte range 复制，软换行不会被错误写成真实换行；输出使用 Crossterm OSC 52。pager 的区域永远止于最底行之上，因此 footer 在打开期间仍可见。

鼠标复制刻意交给终端：进入 pager 后事件循环发送 `DisableMouseCapture`，普通拖拽由本地终端、SSH 或 tmux 解释；关闭 pager 后发送 `EnableMouseCapture`，恢复 Canvas 点击和拖拽。键盘 `y` 则通过 Crossterm 的 OSC 52 命令复制，不引入 X11、Wayland、macOS 或 Windows 专用 clipboard crate；终端或 tmux 禁用 OSC 52 时会在 pager title 显示失败。鼠标捕获切换不退出 raw mode 或备用屏幕。

## 外部编辑器生命周期

完整 `ec` 产生 `EditConfig` interaction。协调层先调用与正常退出相同的 `restore`，让编辑器获得 cooked terminal、主屏幕、鼠标和光标；`config_editor` 优先读取 `$EDITER`，回退 `$EDITOR`，用 `Command` 直接传递 `.cgraph.toml` 路径并同步等待。无论启动或退出是否成功，协调层都先 `resume` raw mode、备用屏幕和鼠标捕获，再把错误写入 footer。

只有零退出状态才调用严格 `ProjectConfig::load`。成功后 `App::reload_symbol_filter` 为 `known_graph` 中所有 `Loaded` / `Loading` 分支生成 `CachePolicy::Refresh` 请求，同时把 `filters.workspace_only` 应用到当前 LSP workspace-symbol 和 hierarchy client；成功空缓存也必须刷新，才能在放宽过滤规则后发现新关系。新 request id 会拒绝编辑期间排队的旧结果。外部编辑器 I/O 不进入 App，配置 loader 不依赖终端，图刷新也不依赖进程 API。

Tree-sitter fallback 在 `main` 中完成语言检测和 grammar/query 初始化，通过同一个通用状态入口报告 working、ready 和 error；TUI 不解释 Tree-sitter API。第一次搜索或展开的索引时间体现在对应 modal/branch 的 loading 状态，Tree-sitter hierarchy 成功后的语法级置信度由 App 写入消息摘要与历史。

## IPC command 编排

TUI 接收可选的 `IpcCommand` receiver，并在每轮 render 前 drain 已到达请求。每条 `focus_symbol` 都先转换为公共 `SymbolIdentity`，再调用 UI 无关的 `App::focus_symbol`；成功/失败映射为同 request id 的 `accepted` / `error`，由随命令携带的 responder 返回原客户端。TUI 不读取原始 JSON、不持有 Unix stream，也不让 socket task 直接修改 widget 或 `RelationGraph`。

外部聚焦成功会按产品语义选择、固定并居中目标，和用户接受搜索结果一致；它不会自动创建 hierarchy task。响应发送失败写入消息摘要与历史，不能终止事件循环。command channel 与每客户端 writer queue 都有界，事件循环只做同步状态迁移，不等待 socket 写 I/O。

## 同步事件循环与异步查询

Crossterm 当前通过同步 `poll/read` 驱动，而 LSP 查询运行在 Tokio task 中。两者使用一个标准库 channel 桥接：

```text
open ac/at or edit query -> replace debounce task -> workspace/symbol(query)
LSP result -> result channel -> App request-id check -> local fuzzy ranking -> render
tl/tr or side button -> one hierarchy task -> result channel -> branch request-id check
r -> incoming + outgoing tasks -> result channel -> independent branch request-id checks
```

每个文本变化都会生成包含完整 query 的新 `SearchRequest`。TUI 只保留一个 Tokio task：任务先等待 200 ms，若期间收到新输入就被 abort，因此快速连续输入不会击穿到语言服务器。已经显示的候选会立刻按新文本在本地重新评分，避免防抖期间列表与输入完全脱节。

App 在安排新请求时进入 `Debouncing`，状态行显示 `Waiting for typing pause…`。task 完成 sleep 后先通过结果 channel 发送 `Started(request_id)`，事件循环确认 id 仍是当前请求后才进入 `Loading` 并显示 `Searching workspace symbols…`；随后 task 才调用统一 provider client。完成事件仍携带相同 id，因而开始和结束状态都不能被旧任务污染。

若 task 已经越过防抖并发出了 JSON-RPC 请求，abort 会让请求 future 被丢弃，Fetch 层随后发送 `$/cancelRequest`。request id 在整个 App 生命周期内单调变化，只有当前 id 的结果才会接收；这是为不严格遵守取消的 server 保留的第二道防线。关闭弹窗同样 abort 当前 task。

本地模糊匹配忽略大小写并采用有序子序列语义。单段查询匹配符号名；多段查询会先尝试把完整文本匹配符号名，失败后用第一段匹配符号名、其余部分匹配 container/path。排序优先级为精确匹配、前缀、连续子串、紧凑子序列，同分时按符号名稳定排序。这个二次评分不会替代 server 查询，而是对 provider 返回结果建立稳定的 TUI 顺序。算法故意留在 App 而非渲染代码中，以便无终端单元测试和未来替换 matcher。

## call/type 结果过滤

workspace symbol 响应包含多种符号。TUI 当前根据公共候选中的 `SymbolKind` 做初步过滤：call 搜索接受 function/method/constructor，type 搜索接受 class/interface/struct/enum/type parameter。provider 已经负责语言适配后的限定名，再交给 App 做项目配置过滤与本地模糊排序；列表不会重复显示已经包含在限定名中的 container 标签。

候选结构已经提升到 Fetch 公共层，并由 LSP/Tree-sitter client 共同返回。`SymbolKind` 仍沿用 LSP 枚举作为跨 provider 分类；如果未来增加无法自然映射的分析器，应再引入项目自己的 symbol-kind，而不是让 TUI 增加 provider 分支。

## 画布布局与连线

画布使用底栏以上的完整区域，不再绘制最外层 `cgraph` 边框。左上角以无边框标题显示 `CALL GRAPH`；当当前选择拥有精确 `SourceLocation` 时，标题改为该节点的文件 URI。标题不参与节点世界布局，因此选择、展开和拖拽不会因为标题发生额外位移。

画布先从 RelationGraph 的 anchors 和 expanded branches 生成可见图，再对可见图计算强连通分量。SCC 收缩后的 DAG 使用最长前驱路径分配水平 rank；caller/parent 的规范 source 位于左侧，callee/child 的 target 位于右侧。同一 SCC 内部、自环或布局后不满足 source-left-of-target 的边标记为非前向边。

同一 rank 的节点按稳定发现顺序纵向排列，不再为了当前选择交换同列节点。每列宽度取该列节点“固定最小宽度”和“符号 Unicode 显示宽度 + 按钮/边框”的最大值，保证相邻列以及同列节点矩形不重叠，并保证 `Class::method` 在终端容得下时不会被内部固定宽度截断。多个不连通组件共享 rank 系统但拥有不同纵向槽位。稳定相对布局完成后，所有 placement 再统一平移，使当前选择中心成为世界原点；切换 selection 只改变这一统一平移，不改变任意节点对的相对几何。

第二阶段以画布中心为投影原点，加上 App 的 viewport 偏移。投影保留有符号的完整 `ProjectedRect` 和无符号的 `visible_slot`：前者描述组件没有被裁剪时应在屏幕上的位置，后者只描述它与 viewport 的交集。只要交集非空，节点就保留在布局快照中；完全离屏后才排除。terminal resize 只改变投影与交集，不改变世界坐标或节点身份。

节点不能直接把 `visible_slot` 交给普通 Ratatui widget，因为 widget 会把这个较小矩形当成新的完整布局区域，在裁剪边界重新绘制边框、重新居中文本。`CanvasNodeWidget` 因此先在从 `(0, 0)` 开始、尺寸等于完整 slot 的局部 `Buffer` 中绘制节点和按钮，再按 `ProjectedRect` 到屏幕的坐标映射复制真实相交单元。只复制节点主体与按钮占用的局部区域，避免局部 buffer 的空白擦除先绘制的连线。这一实现同时保留正确边框切片、样式和命中几何。

渲染、鼠标事件和键盘导航都调用 `canvas_layout`，一次得到包含节点框、左右按钮和已路由边的 `CanvasLayoutSnapshot`。普通前向边使用单线字符和暗灰色；循环、自环和其他非前向边使用黄色双线，反向目标用箭头，自环用 `↺`。边先渲染，节点和按钮后渲染，保证端点不会破坏控件。节点框只显示符号名，不重复写 `call` / `type`。单击节点只改变主选择；单击按钮先选中节点，再独立 toggle 对应方向。

连线不在逐 segment 写入时直接合并字符。`polyline_connection` 先为每个 cell 累积 `LEFT/RIGHT/UP/DOWN` 方向，再一次性映射到单线圆角、单线 T 型、双线方角或对应交汇字符，因此同一条边的转弯不会被误判为 `┼`。普通边在目标按钮前一个 cell 写入 `▶`，避免随后绘制的节点覆盖方向语义。

`CanvasConnections` 在渲染阶段按屏幕坐标聚合不同 edge identity。只有不同边的水平和垂直方向在同一 cell 出现才算真实交叉，并使用加粗洋红样式；一条边自己的 bend 不高亮。普通交叉映射为 `┼`，双线水平/单线垂直映射为 `╪`，单线水平/双线垂直映射为 `╫`，其余特殊交汇使用双线族。这既让颜色终端更醒目，也让无色终端保留轴向语义。默认字形全部来自标准 Unicode，不依赖 Nerd Font 私用区；未来 glyph profile 可以替换装饰，但不能移除默认 profile 的结构语义。

节点方框和边使用不同的可见性边界。`canvas_layout` 为全部世界 placement 生成可以为负数或超出 `u16` 的 `ProjectedNodePlacement`，边路由始终从这份完整集合查找端点；只有节点 widget 列表按 `visible_slot` 过滤。`polyline_connection` 对每个有符号正交 segment 先与 viewport 求交，再枚举交集内的坐标并保留指向屏外的方向 bit。这样端点方框完全离屏时线段仍可见，同时极远的屏外节点不会造成与距离成正比的循环或整数下溢。

拖拽手势状态由事件循环局部的 `CanvasDragState` 保存，而不是写入 App：左键按下节点主体或画布空白处时记录当前位置，后续每个 `Drag(Left)` 将相邻事件的坐标差累积到 viewport，因此内容和指针同向移动；松开左键、进入搜索 modal 或按下画布外区域都会清除锚点。左右 toggle 按钮在命中后明确不建立锚点，避免展开操作被解释成平移。拖拽节点表示移动观察窗口，不是修改单个节点的世界位置。

同一局部状态还记录按下节点、是否发生拖拽和最近一次完整点击。只有同一 `NodeId` 在 500 ms 内完成两次 down/up 且两次都没有 drag，才生成 `OpenLocation` interaction；按钮、画布外释放、空白点击和拖拽会清除点击序列。TUI 只把精确 `SourceLocation` 交给可选 `IpcEventSender`，不直接持有 listener 或 client stream。发送失败转成消息摘要并进入历史，不能让 IPC 故障退出 TUI。

选择和 toggle 不能直接依赖“selection 位于世界原点”的布局结果，否则目标节点会突然跳到画布中心。控制器用 `with_stable_node_position` 在状态迁移前后读取操作节点的世界中心，并把差值反向累加到 viewport。鼠标选择、空间键盘导航、缓存 toggle、首次异步请求以及 hierarchy 完成事件都经过同一入口。锚点使用中心而不是左上角，是因为 CLI 临时符号在 prepare 后可能解析成长限定名并改变节点宽度；此时用户关注的视觉位置仍保持稳定。搜索接受结果是明确例外，它会重置 viewport，把新 anchor 放回中心。

首次 toggle 会立即把目标分支置为 `Loading` 并生成 hierarchy task；其他分支不受影响。裸 `r` 生成包含两个 `CachePolicy::Refresh` 请求的批量 interaction，事件循环仍为每个方向启动独立 task。刷新不会先清空缓存或递归查询后代；失败保留旧 neighbors，连续刷新由分支 request id 拒绝迟到结果。按钮用 `[+]`、`[~]`、`[-]`、`·`、`[!]` 区分未加载、加载中、展开、成功空结果和失败。

方向键和 `h/j/k/l` 使用相同布局快照进行空间导航。算法把候选限制在目标方向半平面，按欧氏距离平方、垂直偏移和主方向距离排序；同分时使用稳定的 `NodeId`。导航只会命中当前可见节点，目标方向没有候选时不修改选择。前缀状态在单键导航前处理，所以 `tl` 中的 `l` 不会触发向右移动。

有向图布局、无限画布、投影和整体拖拽已经接入。画布仍有以下后续工作：

- layout cache：图状态未变化时复用 SCC、rank 和世界布局，减少大图每帧开销。
- modal stack：搜索、保存和错误弹窗统一管理，避免大量 `Option<ModalState>` 字段。

布局器不应直接发查询；点击展开应先生成 App command，再由协调层决定是否命中缓存或启动异步请求。

## 已知 TODO

- 支持多个 workspace symbol providers 并发查询，再像 VS Code 一样跨 provider 去重。
- 在结果 UI 中标出符号名与 container/path 的模糊命中字符。
- 将 `ListState` 的滚动 offset 持久化，统一键盘与鼠标对长列表的映射。
- 支持粘贴事件、宽字符光标位置和超小终端降级布局。
- 在窄终端中为分析状态提供单行降级模式，而不是只能完全隐藏。
- 为 terminal init 中途失败增加更强的恢复守卫。
- 把事件映射拆成可单元测试的 controller，减少 `tui/mod.rs` 体积。
