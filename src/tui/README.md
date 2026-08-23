# TUI 层设计

TUI 层把终端事件转换为 App 状态迁移，并把 App 渲染为 Ratatui widgets。业务不变量应尽量留在 App/State，TUI 只处理输入语义、坐标映射和视觉表现。

代码按变化原因拆分：`mod.rs` 保存事件循环、键鼠控制器、搜索弹窗和顶层渲染编排；`canvas.rs` 是无异步 I/O 的画布几何模块，集中负责世界布局、viewport 投影、完整矩形碰撞与连接线单元。这样修改 LSP 请求生命周期不会触碰布局算法，调整节点尺寸也不需要穿过 modal/controller 代码。

相关产品规范：[REQ-2 分析后端状态](../../requirements/REQ-2-analysis-status/README.md)、[REQ-4 画布与导航](../../requirements/REQ-4-canvas-navigation/README.md)、[REQ-5 符号与树管理](../../requirements/REQ-5-symbol-management/README.md)。本文件解释实现理由，不替代这些需求的验收条件。

## 当前模式

事件处理分为两个模式：

- Canvas 模式：`a` 后接 `c` / `t` 打开 call/type 搜索框；`t` 后接 `l` / `r` 独立 toggle 左右分支；`d` 后接 `d` / `p` / `n` 删除树或方向分支；`q` 或 `Esc` 退出。
- Search modal 模式：普通字符编辑查询，方向键或 `Ctrl-n` / `Ctrl-p` 选择，回车确认，`Esc` 关闭。

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

footer 固定为一行，左侧约占 3/5 显示当前快捷键或 hierarchy 错误，右侧约占 2/5 显示 `backend · phase [percentage] · message`。两个 `Paragraph` 各自在自己的 `Rect` 内截断，因此长状态不会覆盖快捷键，也不再占用画布。LSP、Tree-sitter、phase 和消息使用不同颜色，但文本本身必须足以表达状态，不能只依赖颜色。

Tree-sitter fallback 在 `main` 中完成语言检测和 grammar/query 初始化，通过同一个通用状态入口报告 working、ready 和 error；TUI 不解释 Tree-sitter API，也不会把 parser ready 当成 workspace symbol ready。

## 同步事件循环与异步查询

Crossterm 当前通过同步 `poll/read` 驱动，而 LSP 查询运行在 Tokio task 中。两者使用一个标准库 channel 桥接：

```text
open ac/at or edit query -> replace debounce task -> workspace/symbol(query)
LSP result -> result channel -> App request-id check -> local fuzzy ranking -> render
tl/tr or side button -> hierarchy task -> hierarchy result channel -> branch request-id check
```

每个文本变化都会生成包含完整 query 的新 `SearchRequest`。TUI 只保留一个 Tokio task：任务先等待 200 ms，若期间收到新输入就被 abort，因此快速连续输入不会击穿到语言服务器。已经显示的候选会立刻按新文本在本地重新评分，避免防抖期间列表与输入完全脱节。

App 在安排新请求时进入 `Debouncing`，状态行显示 `Waiting for typing pause…`。task 完成 sleep 后先通过结果 channel 发送 `Started(request_id)`，事件循环确认 id 仍是当前请求后才进入 `Loading` 并显示 `Searching workspace symbols…`；随后 task 才调用 LSP client。完成事件仍携带相同 id，因而开始和结束状态都不能被旧任务污染。

若 task 已经越过防抖并发出了 JSON-RPC 请求，abort 会让请求 future 被丢弃，Fetch 层随后发送 `$/cancelRequest`。request id 在整个 App 生命周期内单调变化，只有当前 id 的结果才会接收；这是为不严格遵守取消的 server 保留的第二道防线。关闭弹窗同样 abort 当前 task。

本地模糊匹配忽略大小写并采用有序子序列语义。单段查询匹配符号名；多段查询会先尝试把完整文本匹配符号名，失败后用第一段匹配符号名、其余部分匹配 container/path。排序优先级为精确匹配、前缀、连续子串、紧凑子序列，同分时按符号名稳定排序。这个二次评分不会替代 server 查询，而是对 provider 返回结果建立稳定的 TUI 顺序。算法故意留在 App 而非渲染代码中，以便无终端单元测试和未来替换 matcher。

## call/type 结果过滤

workspace symbol 响应包含多种符号。TUI 当前根据 `SymbolKind` 做初步过滤：call 搜索接受 function/method/constructor，type 搜索接受 class/interface/struct/enum/type parameter。method/constructor 会使用 `containerName` 规范化为 `Container::name`，再交给 App 做项目配置过滤与本地模糊排序；列表不会重复显示已经包含在限定名中的 container 标签。

这是暂时放在 TUI 的适配逻辑。长期应由 Fetch 层返回已经归一化的候选类型，因为 Tree-sitter 不会产生 LSP `SymbolKind`。

## 画布布局与连线

画布布局分为两个明确阶段。第一阶段在与终端大小无关的有符号世界坐标中放置所有根及已展开节点：当前选择所在的根位于世界原点，其他根沿稳定行排列；incoming/outgoing 孩子分别位于父节点左/右列。节点槽宽度取固定最小宽度与“符号 Unicode 显示宽度 + 按钮/边框”中的较大值，保证 `Class::method` 在终端容得下该节点时不会被内部固定宽度截断。候选槽使用包含按钮在内的完整矩形与所有已占用槽做相交测试，冲突时选择距离期望纵坐标最近的空边界。不能只比较左上角，否则不同宽度或半行偏移的同列节点仍会发生覆盖。

第二阶段以画布中心为投影原点，加上 App 的 viewport 偏移，再裁剪成 Ratatui `Rect`。初版只投影完整进入画布的节点框，避免在终端边缘绘制残缺边框和按钮；屏外节点仍存在于世界布局，平移后可重新进入可见集合。terminal resize 只改变投影与裁剪，不改变世界坐标或节点身份。

渲染、鼠标事件和键盘导航都调用 `canvas_layout`，得到节点框与左右按钮的同一组屏幕 `Rect`。`canvas_edges` 再从可见父子关系生成正交线单元，先画线、后画节点和按钮，保证端点不会破坏控件。节点框只显示符号名，不重复写 `call` / `type`。单击节点只改变主选择；单击按钮先选中节点，再独立 toggle 对应方向。`tl` / `tr` 使用相同的 App 状态迁移，不允许裸 `t` 同时展开两侧。

拖拽手势状态由事件循环局部的 `CanvasDragState` 保存，而不是写入 App：左键按下节点主体或画布空白处时记录当前位置，后续每个 `Drag(Left)` 将相邻事件的坐标差累积到 viewport，因此内容和指针同向移动；松开左键、进入搜索 modal 或按下画布外区域都会清除锚点。左右 toggle 按钮在命中后明确不建立锚点，避免展开操作被解释成平移。拖拽节点表示移动观察窗口，不是修改单个节点的世界位置。

首次 toggle 会立即把目标分支置为 `Loading` 并生成 hierarchy task；其他分支不受影响。按钮用 `[+]`、`[~]`、`[-]`、`·`、`[!]` 区分未加载、加载中、展开、成功空结果和失败。完成事件携带分支 request id，删除或重试后的旧结果会被 App 拒绝。

方向键和 `h/j/k/l` 使用相同布局快照进行空间导航。算法把候选限制在目标方向半平面，按欧氏距离平方、垂直偏移和主方向距离排序；同分时使用稳定的 `NodeId`。导航只会命中当前可见节点，目标方向没有候选时不修改选择。前缀状态在单键导航前处理，所以 `tl` 中的 `l` 不会触发向右移动。

无限画布、投影和整体拖拽已经接入。画布仍有以下后续工作：

- focus/selection：键盘空间导航与语义相同节点的同步高亮。
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
