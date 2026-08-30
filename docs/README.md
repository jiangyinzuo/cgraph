# cgraph 用户文档

这里存放面向使用者的文档。产品愿景记录在仓库根目录的 `DESIGN.md`，详细产品需求位于 `requirements/`，内部实现说明位于 `src/`；使用 cgraph 通常不需要阅读内部文档。

## 阅读顺序

1. [快速上手](getting-started.md)：安装、准备语言服务器并完成第一次符号搜索。
2. [命令与交互](commands.md)：命令行选项、键盘和鼠标操作。
3. [显示与终端字体](display-and-fonts.md)：节点裁剪、连线图例和字体建议。
4. [项目配置](project-configuration.md)：过滤项目中的高噪声符号。
5. [导出关系图](export.md)：保存简洁、稳定且便于阅读的文本。
6. [编辑器联动](editor-integration.md)：通过 Unix socket 在节点与 Neovim 之间双向定位。
7. [语言服务器](language-servers.md)：自动检测、显式配置和索引行为。
8. [故障排查](troubleshooting.md)：搜索为空、LSP 无法启动和终端恢复问题。

## 当前实现状态

已可使用：

- 启动空画布，或从 `call` / `type` 命令创建初始 anchor。
- 自动检测或显式启动 stdio LSP server。
- 使用 `ac` / `at` 异步加载项目符号，并在本地即时模糊筛选。
- 使用项目根目录的 `.cgraph.toml` 过滤搜索与 hierarchy 中的高噪声符号。
- 使用 `ec` 调用 `$EDITER` 编辑并重载项目配置，然后刷新全部已加载图分支。
- 使用 `?` 打开可滚动的完整键鼠帮助；常驻 footer 只显示高频入口。
- 通过键盘或鼠标选择搜索结果并创建或重定位 anchor。
- 在最底栏右侧查看 LSP 连接、后台 progress、警告、错误或断开状态。
- 通过 `g<` 查看 LSP 初始化/空查询诊断，并从默认 `/tmp` 会话日志读取 server stderr。
- 在没有 LSP 时使用 Tree-sitter；LSP 缺少某种 hierarchy 能力时按查询粒度静态回退。
- 同时显示多个图入口和共享节点，并用 `dd` / `dp` / `dn` 取消 anchor 或清除分支。
- 使用 `tl` / `tr` 懒加载 call/type hierarchy；优先使用已声明能力的 LSP，否则使用同语言 Tree-sitter 后备。菱形关系复用节点，循环/反向边用特殊双线标记。
- 使用 `r` 同时刷新当前节点左右一层关系，并保留仍存在节点的深层展开状态。
- 拖拽节点或画布空白处平移无限画布，查看终端之外的节点。
- 在画布边缘保留节点的真实可见切片，并用圆角、箭头及高亮交叉字符显示连线方向。
- 使用 `w` 把全部已知可达关系保存为简洁文本，且绝不覆盖已有目标。
- 使用 `--ipc-socket` 启动双向编辑器服务：双击节点广播零基 UTF-16 源码位置，外部客户端可按符号或精确位置聚焦 call/type anchor。

文档中带“计划”标记的行为不能视为当前版本已经支持。完整状态以[需求索引](../requirements/README.md)为准。
