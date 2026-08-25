# REQ-9-2：在 TUI 中编辑并重载配置

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-9` |
| 状态 | `Implemented` |
| 优先级 | `P1` |
| 目标版本 | `0.1` |

## 目标

用户无需退出 cgraph，即可编辑当前 workspace 的 `.cgraph.toml` 并让新过滤规则作用于当前关系图。

## 需求

- Canvas 模式的 `ec` 前缀命令使用 `$EDITER` 指定的本地编辑器打开 `<workspace>/.cgraph.toml`；为兼容通用工具链，未设置 `$EDITER` 时回退到标准 `$EDITOR`。
- 配置文件不存在时，在启动编辑器前以安全的 `create_new` 方式创建最小有效模板，绝不覆盖竞态中出现的文件。
- 启动外部编辑器前恢复 raw mode、备用屏幕、鼠标捕获和光标；编辑器退出后重新进入 TUI，即使编辑器启动或退出失败也保持终端可用。
- 只有编辑器成功退出且配置重新校验成功时替换当前过滤器和 workspace 范围策略；失败保留上一份有效配置，在倒数第二行显示原因并写入消息历史。
- 重载成功后刷新所有已经加载过的 incoming/outgoing 图分支；未加载分支仍保持惰性，显式 anchors 保留。

## 验收条件

- `e` 只进入前缀状态，只有完整 `ec` 才启动编辑器。
- `$EDITER` 优先于 `$EDITOR`；两者都缺失时不离开 TUI并显示可诊断错误。
- 编辑器获得项目配置的准确路径，非零退出状态不会应用配置。
- 合法配置在同一会话内生效，已加载的成功空分支也会刷新，以便放宽规则后恢复先前被过滤的关系。
- 修改 `filters.workspace_only` 后，后续搜索立即使用新范围，已有 loaded/loading 分支通过 refresh request 重新查询。
- 重载产生新的 request id，编辑期间完成的旧 hierarchy 结果不能覆盖刷新结果。
- 没有分析 provider 时配置仍会重载，footer 明确说明没有可刷新的后端。

## 关联文档

- [项目配置](../../docs/project-configuration.md)
- [命令与交互](../../docs/commands.md)
- [TUI 内部设计](../../src/tui/README.md)
- [测试设计](../../src/testing/README.md)

## 实现证据

- `src/tui/config_editor.rs` 选择编辑器、创建最小配置并把准确路径传给真实子进程。
- `src/tui/mod.rs` 在启动编辑器前后执行终端 restore/resume，成功后严格重载配置并安排刷新任务。
- `App::reload_symbol_filter` 为所有已加载或正在刷新的可达分支生成新的 refresh request；TUI 同时更新 LSP client 的 workspace 范围策略，沿用过滤、去重和迟到响应拒绝语义。
- 自动测试覆盖 `$EDITER` / `$EDITOR` 选择、真实子进程参数、安全模板、`ec` 前缀、成功空分支刷新、过滤生效、anchor 保留和旧 request id 失效。
