# REQ-8-1：Pyrefly 默认 Python LSP

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-8` |
| 状态 | `Implemented` |
| 优先级 | `P1` |
| 目标版本 | `0.1` |

## 目标

Python 工作区无需额外参数即可使用 Pyrefly 的 workspace symbol、call hierarchy 和 type hierarchy，并保持 Python 惯用的限定名。

## 需求

- 浅层检测到 `pyproject.toml`、`pyrefly.toml`、`setup.py`、`requirements.txt` 或根目录 `.py` 文件时，默认语言服务器为 Pyrefly。
- 自动检测或显式 `--lsp pyrefly` 都必须启动 `pyrefly lsp`；重复的 `--lsp-arg` 追加在 `lsp` 子命令之后。
- 用户可以用 `--lsp pylsp` 或其他 stdio LSP 程序显式覆盖默认值。
- Python 项目也可以在 `.cgraph.toml` 的 `[lsp]` 段配置 `command` 和 `args`；该项目配置优先于自动检测，但低于一次性的 CLI 覆盖。
- 仅在 server capability 声明支持时使用标准 workspace symbol、call hierarchy 和 type hierarchy，不实现 Pyrefly 私有 hierarchy 协议。
- Pyrefly call hierarchy 的模块限定 `detail` 只在已识别的 Python adapter 中解释；方法显示为 `Class.method`，不把模块路径或任意 detail 当成类名。
- 没有精确位置的 Python CLI 根同时接受 `Class.method` 和通用 `Class::method` 输入，并用末级方法名解析 workspace symbol。
- initialize 完成后，从项目内选择一个受限大小的 Python 源文件发送标准 `textDocument/didOpen`，触发 Pyrefly 的 lazy workspace index；shutdown 前发送对应的 `textDocument/didClose`，不发送 change 或 save。
- 索引引导扫描必须跳过隐藏目录、构建目录、虚拟环境、`node_modules`、`__pycache__` 和符号链接，并限制扫描条目数与文件大小；找不到安全文件时保持 LSP 已连接，不伪造索引完成状态。
- Pyrefly 无法启动时继续使用现有 Tree-sitter 回退，并在底栏保留可诊断错误。

## 验收条件

- Python 自动检测产生的进程参数以 `pyrefly`、`lsp` 开头。
- `--lsp pyrefly --lsp-arg=--indexing-mode --lsp-arg=lazy-blocking` 保持 `lsp` 在所有用户参数之前。
- 显式 `--lsp pylsp` 不会错误附加 Pyrefly 子命令。
- 通过可执行文件名或 initialize 返回的 `pyrefly-lsp` server name 都能选择 Pyrefly 名称适配器。
- `worker.Worker.run` detail 归一化为 `Worker.run`；普通模块函数和路径样式 detail 不被错误限定。
- cgraph 继续在弹窗打开和每次输入后发送第一项 query，后续项只在本地模糊筛选；Pyrefly 当前对少于 3 个字符的 workspace-symbol 查询返回空结果属于 server 限制，客户端不伪造候选。
- 环境中存在 `pyrefly` 时，真实最小 Python workspace 能搜索到 `helper`，其 incoming call hierarchy 包含 `Worker.run`；命令不存在时测试安全跳过。

## 当前实现与差距

默认启动、命令组装、server-name 检测、Python 点号限定名、CLI 根末级解析、标准 configuration 响应和受控文档索引引导已经实现。Pyrefly 默认的后台索引模式声明 call/type hierarchy capability；用户把 `--indexing-mode` 设为 `none` 时，相关 capability 会由 server 关闭并按现有错误边界呈现。

Pyrefly 的 workspace-symbol 索引当前主要返回可搜索的导出符号，并自行要求至少 3 个查询字符。cgraph 不枚举或猜测 server 没有返回的方法；需要完整语法级项目候选时仍可显式选择 Tree-sitter 模式。

## 实现证据

- Python 默认检测和命令组装测试位于 `src/main.rs`。
- 已知 server 命令、configuration、Python 根解析和条件执行的真实 Pyrefly 集成测试位于 `src/fetch/lsp.rs`。
- 安全的索引引导扫描与 bootstrap document 位于 `src/fetch/lsp/pyrefly.rs`。
- Pyrefly 名称方言测试位于 `src/fetch/lsp/symbol_names.rs`。
- 用户配置和 provider 限制记录在 `docs/language-servers.md`。
