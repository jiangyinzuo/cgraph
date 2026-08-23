# ctree 用户文档

这里存放面向使用者的文档。产品愿景记录在仓库根目录的 `DESIGN.md`，详细产品需求位于 `requirements/`，内部实现说明位于 `src/`；使用 ctree 通常不需要阅读内部文档。

## 阅读顺序

1. [快速上手](getting-started.md)：安装、准备语言服务器并完成第一次符号搜索。
2. [命令与交互](commands.md)：命令行选项、键盘和鼠标操作。
3. [项目配置](project-configuration.md)：过滤项目中的高噪声符号。
4. [语言服务器](language-servers.md)：自动检测、显式配置和索引行为。
5. [故障排查](troubleshooting.md)：搜索为空、LSP 无法启动和终端恢复问题。

## 当前实现状态

已可使用：

- 启动空画布，或从 `call` / `type` 命令创建初始 anchor。
- 自动检测或显式启动 stdio LSP server。
- 使用 `ac` / `at` 异步加载项目符号，并在本地即时模糊筛选。
- 使用项目根目录的 `.ctree.toml` 过滤搜索与 hierarchy 中的高噪声符号。
- 通过键盘或鼠标选择搜索结果并创建或重定位 anchor。
- 在最底栏右侧查看 LSP 连接、后台 progress、警告、错误或断开状态。
- 在没有 LSP 时初始化已识别语言的 Tree-sitter grammar/query 并显示状态。
- 同时显示多个图入口和共享节点，并用 `dd` / `dp` / `dn` 取消 anchor 或清除分支。
- 使用 `tl` / `tr` 懒加载 LSP call/type hierarchy；菱形关系复用节点，循环/反向边用特殊双线标记。
- 拖拽节点或画布空白处平移无限画布，查看终端之外的节点。

尚未实现：

- 刷新、导出和 Neovim IPC。
- Tree-sitter workspace symbol 与 hierarchy 查询回退。

文档中带“计划”标记的行为不能视为当前版本已经支持。完整状态以[需求索引](../requirements/README.md)为准。
