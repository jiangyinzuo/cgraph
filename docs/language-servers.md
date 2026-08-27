# 语言服务器

cgraph 以 LSP 客户端身份启动一个通过 stdin/stdout 通信的语言服务器。当前会话只启动一个 server，因此复杂多语言工作区需要显式选择。

当前每次启动 cgraph 都会启动自己的语言服务器；同一次 TUI 会话中的所有搜索会复用该进程，但退出后不会保留 rust-analyzer 的内存索引。这一阶段优先保证会话隔离和确定的 shutdown 生命周期，尚未实现常驻 workspace daemon。

TUI 最底栏右侧的状态摘要展示该会话的连接和后台进度，左侧同时保留快捷键提示。它与 `ac` / `at` 搜索弹窗中的单次查询状态分开维护。

如果没有检测到 LSP、显式使用 `--no-lsp`，或 LSP 启动失败，cgraph 会尝试根据工作区浅层标志初始化 Rust、C、C++ 或 Python 的 Tree-sitter grammar 和 tags query。成功时底栏显示 `Tree-sitter: <language> · Ready`；初始化失败显示 Error。LSP 正常运行时也会初始化同语言的轻量 hierarchy 后备，但不会替代底栏中的 LSP 主状态，也不会提前扫描项目。第一次实际使用 Tree-sitter 搜索或 hierarchy 回退时才在 blocking task 中惰性建立项目静态索引，后续查询复用该索引。

## 自动检测

如果没有传入 `--lsp` 或 `--no-lsp`，cgraph 按以下顺序检查工作区根目录：

1. `Cargo.toml` → `rust-analyzer`
2. `compile_commands.json`、`CMakeLists.txt` 或根目录 C/C++ 源文件/头文件 → `clangd`
3. `pyproject.toml`、`pyrefly.toml`、`setup.py`、`requirements.txt` 或根目录 `.py` 文件 → `pyrefly lsp`

检测是有意保持简单的启动便利功能，不会递归扫描整个仓库，也不会解析编辑器配置。monorepo、多语言仓库或自定义 server 应显式配置。

## 显式配置

推荐将团队统一使用的 server 写入 workspace 根目录的 `.cgraph.toml`：

```toml
[lsp]
name = "clangd"
command = "clangd"
args = ["--background-index"]
file_extensions = ["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"]
```

`name` 选择 server profile/语言方言，`command` 是实际可执行文件（可以是包装脚本或绝对路径），`args` 是按顺序传递的参数数组，不经过 shell。`file_extensions` 是该 profile 的项目文件后缀列表，用于选择触发 server 索引的 bootstrap 文档；写 `hpp` 或 `.hpp` 均可，配置会规范化为小写后缀，但不接受路径和 `*`。省略时，内置 clangd profile 会同时覆盖 C/C++ 源文件与 `h/hh/hpp/hxx` 头文件；Rust 与 Pyrefly 分别使用 `rs` 和 `py/pyi`。省略 `[lsp]` 时，cgraph 使用内置默认 profile：`Cargo.toml` 选择 `rust-analyzer`，C/C++ 标志选择 `clangd`，Python 标志选择 `pyrefly lsp`。项目配置适合提交到版本库；通过 `ec` 修改后，LSP 配置会在下次启动生效。

```bash
cgraph --lsp rust-analyzer --workspace /work/project
cgraph --lsp clangd --lsp-arg=--background-index --workspace /work/project
cgraph --lsp pyrefly --workspace /work/project
# 仍可显式选择其他 Python LSP：
cgraph --lsp pylsp --workspace /work/project
```

`--lsp` 当前接受可执行程序名，不解析一整段 shell 命令。每个参数都要单独写成 `--lsp-arg`，这样可以避免 shell 拼接和转义歧义；CLI 参数优先于 `.cgraph.toml`。Pyrefly 是已知例外：选择可执行文件 `pyrefly` 时，cgraph 自动把 `lsp` 作为第一个参数，用户不应再手工添加；例如可用 `--lsp pyrefly --lsp-arg=--indexing-mode --lsp-arg=lazy-blocking` 覆盖其索引模式。

## 标准协议边界与配置边界

cgraph 的 JSON-RPC transport、workspace symbol、document symbol 和 hierarchy 请求遵循标准 LSP，不根据 clangd、rust-analyzer 或其他 server 改写请求语义。语言服务器返回空 workspace symbol 列表时，客户端不能仅凭协议判断“确实没有匹配项”还是“服务端索引尚未完成”；索引进度属于 server 实现和状态通知能力，不是 `workspace/symbol` 的额外协议参数。

当前版本仍保留少量历史兼容逻辑，例如为没有编辑器当前 buffer 的 cgraph 会话向 clangd/Pyrefly 打开一个受限 bootstrap 文档。这些逻辑属于内置 server profile，而不是通用 LSP 要求。项目配置已经可以声明 server 的名称、程序、参数和项目文件后缀；通用 actor 只负责标准 LSP 生命周期，语言专用行为收敛在可替换 profile 中，用户可以自行配置新的语言服务器而无需修改 Fetch 核心。初始化选项和 bootstrap 策略暂未开放为 TOML 字段。

## 初始化行为

cgraph 会发送：

- 当前进程 id；
- canonicalized workspace URI；
- workspace folder；
- workspace symbol、call hierarchy、type hierarchy、workspace folders 和 configuration 客户端能力；call/type hierarchy 允许 server 动态注册；
- 仅支持 UTF-16 的 position encoding 能力；server 未声明时按 LSP 默认 UTF-16 解释，显式选择其他编码则拒绝会话；
- 标准 `window.workDoneProgress` 客户端能力；
- rust-analyzer 使用的实验性 server status notification 能力；
- 客户端名称与版本；
- 可选 initialization options（当前只在库 API 中配置）。

server 必须在 initialize 结果中声明支持 `workspace/symbol`，否则 cgraph 会保留 TUI，但搜索框显示 LSP 不可用。

## 连接与后台进度

initialize 成功后，底部状态摘要首先显示 `Ready`。这只表示 LSP 会话已经建立，不保证语言服务器已经完成项目加载和符号索引。

cgraph 会持续读取标准 `$/progress` work-done 通知，并显示最近更新的活动任务。多个任务并行时，一个任务结束不会错误地把整体状态切回 `Ready`；最后一个已知任务结束后才恢复就绪。任务提供百分比时会限制在 0–100 后显示。rust-analyzer 的 `experimental/serverStatus` 会转换为相同的 `Ready`、`Working`、`Warning` 或 `Error` 状态。连接结束则显示 `Disconnected`。

并非所有语言服务器都会发送进度通知。没有进度不代表没有后台索引，`Ready` 的准确含义仍受 server 实现限制。cgraph 当前也不从 stderr 推断状态。

## 索引与查询时机

语言服务器可能在 initialize 后继续索引。cgraph 会在后台处理 `workspace/configuration` 等反向请求，所以索引不会因为 UI 空闲而停住；但大型项目的第一次查询仍可能较慢或暂时为空。

cgraph 采用与 VS Code workspace symbol quick access 相同的查询节奏。打开 `ac` / `at` 或输入发生变化后，会等待约 200 ms；期间再次输入会重置计时。等待结束后，当前完整文本交给当前 provider。LSP 模式通过 `workspace/symbol` 发送给 server，包括空文本；Tree-sitter 模式从惰性项目索引返回全部候选，再由 App 做相同的本地模糊筛选。

UI 在等待阶段显示 `Waiting for typing pause…`。防抖结束后，后台任务先通知 App 请求已经开始，再调用 provider client，此时状态才切换为 `Searching workspace symbols…`。这两个状态分别表示客户端本地等待和实际 provider 查询；Tree-sitter 的第一次查询包含项目索引时间。

新的文本会中止之前的查询任务。如果旧请求已经写入 server，JSON-RPC 层会发送 `$/cancelRequest`；request id 还会作为第二道防线，忽略 server 在取消后仍然返回的旧结果。关闭弹窗也会取消尚未完成的查询。

rust-analyzer 默认只搜索类型。cgraph 在 initialization options 和 `workspace/configuration` 中设置 `kind=all_symbols` 与 `scope=workspace`，使 call 搜索可以获得函数，同时不包含依赖；结果数量保留 rust-analyzer 为逐查询客户端设计的默认 128 项上限。其他 server 直接接收标准查询文本。

provider 返回后，cgraph 会按符号身份去重、按 call/type 所需的 `SymbolKind` 过滤并进行本地模糊评分。默认情况下 LSP 结果还会只保留 URI 位于 canonical workspace 根目录下的符号；项目配置 `[filters].workspace_only = false` 可以关闭这一范围过滤。Tree-sitter 从一开始只扫描项目源文件，并跳过隐藏目录、`target`、`node_modules` 和符号链接。

Pyrefly 自身只在 query 至少有 3 个字符时执行 workspace-symbol 搜索；空文本、1 个字符或 2 个字符会返回空结果。cgraph 仍按统一节奏发送每次完整 query，不在 UI 中制造额外门槛，也不会为 server 未返回的内容伪造候选。Pyrefly 默认的后台索引模式声明标准 call/type hierarchy；使用 `--indexing-mode none` 会由 server 关闭这些能力。Pyrefly hierarchy 中能够确认的方法按 `Class.method` 显示，模块函数保持原名。

为触发 Pyrefly 的 lazy workspace index，cgraph 会只读打开一个受限大小的项目 Python 文件，并在 LSP shutdown 前关闭它；不会发送 change 或 save。文件发现会跳过隐藏目录、构建目录、虚拟环境、依赖目录和符号链接。若项目内没有安全可读的 `.py` 文件，LSP 连接仍会建立，但首次符号查询可能为空。

## Hierarchy 查询

首次展开 call 分支时，cgraph 先发送 `textDocument/prepareCallHierarchy`，再按方向发送 `callHierarchy/incomingCalls` 或 `callHierarchy/outgoingCalls`。type 分支对应 `textDocument/prepareTypeHierarchy` 与 `typeHierarchy/supertypes` / `typeHierarchy/subtypes`。prepare 返回的协议 item 会原样带入第二阶段请求，邻接结果随后归一化为名称、种类和源码位置，并按 resolved identity 写入全局关系图；不同路径发现的同一节点只显示一次。

每个节点的左右分支独立保存 `NotLoaded`、`Loading`、`Loaded` 或 `Failed`。成功结果（包括成功的空结果）会缓存；失败不会伪装成空结果，按相同方向键或点击 `[!]` 可以重试。语言服务器未实现某个标准方法时也会进入明确错误状态。

Tree-sitter 使用同一个方向模型：caller/parent 指向 callee/child。Rust 和 Python 使用 grammar tags 中的直接调用捕获，C/C++ 使用 `call_expression`；Rust trait impl、C++ base class 和 Python superclass 形成类型边，C 没有语言级继承边。只有名称能在项目定义中唯一绑定时才建立关系，方法优先在当前类/impl 内绑定。动态分派、宏展开后的调用、复杂 import/namespace 解析和歧义目标可能省略，因此每次成功结果都会在倒数第二行显示 `syntactic relations only` 并进入消息历史，不能将它等同于 LSP 完整语义。

LSP hierarchy 不采用“先发送再看是否返回 Method not found”的能力探测。call hierarchy 读取 initialize 的静态声明并接受后续动态注册；LSP 3.17 type hierarchy 依赖 `client/registerCapability`，cgraph 会追踪注册与注销。若当前 server 没有声明所需 kind，而工作区语言受 Tree-sitter 支持，该次 hierarchy 查询自动使用静态后备；其他已声明能力仍留在同一个 LSP 会话中。当前 rust-analyzer 不实现标准 type hierarchy，因此 Rust 的 `at` 节点左右展开会走 Tree-sitter，`ac` 搜索和 call 展开仍走 rust-analyzer。

## 独立诊断示例

可以绕过 TUI，直接检查 server 是否能返回 workspace symbols：

```bash
cargo run --example lsp_workspace_symbols -- rust-analyzer LspProvider .
```

参数依次是 server 程序、查询字符串和可选工作区。示例等待两秒再查询，只用于诊断，不代表 TUI 有固定两秒延迟。

也可以直接检查 hierarchy。以下附加的文件、行、列均为一基坐标；省略它们时，示例会先通过 workspace symbol 解析名称：

```bash
cargo run --example lsp_hierarchy -- rust-analyzer call outgoing main . src/main.rs 16 10
```

hierarchy 示例为冷索引预留十二秒；这只是独立诊断工具的等待窗口，TUI 不会固定等待十二秒。

rust-analyzer 冷索引的内部原因、不能直接附加编辑器现有进程的限制，以及未来 daemon/编辑器代理方案，详见维护者文档 [rust-analyzer 生命周期与索引复用设计](../src/fetch/rust-analyzer.md)。
