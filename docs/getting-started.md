# 快速上手

## 环境要求

- Rust 1.88 或更新版本。
- 推荐安装一个支持标准输入输出通信的语言服务器；没有 LSP 时可使用内置 Tree-sitter 静态回退。
- 终端需要支持 raw mode、备用屏幕和基本鼠标事件。

当前常用组合：

| 工作区 | 推荐语言服务器 |
| --- | --- |
| Rust | `rust-analyzer` |
| C/C++ | `clangd` |
| Python | Pyrefly（`pyrefly lsp`） |

语言服务器不随 cgraph 一起安装，请使用系统包管理器、Rust 工具链或对应语言的包管理器安装。Python 工作区需要让 `pyrefly` 位于 `PATH`；cgraph 会自动添加 `lsp` 子命令。也可以使用 `--no-lsp` 明确进入 Tree-sitter 静态模式。

## 安装与运行

### 安装已发布版本

如果已经发布到 crates.io，可以直接安装最新版本：

```bash
cargo install call-graph-cli
cgraph
```

### 从源码运行

```bash
git clone <cgraph-repository>
cd cgraph
cargo run
```

也可以安装当前工作区中的二进制：

```bash
cargo install --path .
cgraph
```

## 第一次搜索

在一个 Rust 项目目录中运行：

```bash
cgraph --workspace .
```

如果目录中存在 `Cargo.toml`，cgraph 会尝试启动 `rust-analyzer`。进入 TUI 后：

- 最底栏右侧显示当前分析后端。`LSP: rust-analyzer · Ready` 表示连接已建立；`Working` 及任务消息表示语言服务器仍在加载项目、索引或执行其他后台工作，左侧同时保留快捷键提示。

1. 输入 `at` 打开 type 搜索框。
2. 直接输入任意长度的文本，例如 `V` 或 `Ve`；不要求先输入两个字符。
3. 停止输入约 200 ms 后，cgraph 会把当前完整文本交给当前分析 provider。继续输入会取消过时查询并重新计时。
4. 防抖期间会显示 `Waiting for typing pause…`；请求真正发出后变为 `Searching workspace symbols…`，完成后显示结果数量。返回结果还会在本地进行不区分大小写的模糊排序。
5. 使用上下方向键选择结果，再按回车把该类型作为 anchor 加入画布。
6. 重复搜索其他符号时，多个 anchor 会同时显示；重新选择已有语义节点会复用它并移回中心，而不重复创建节点。
7. 使用方向键或 `h`、`j`、`k`、`l` 在可见节点间移动，也可以鼠标单击节点；首次按 `tl` / `tr` 会异步加载对应方向的一层关系，成功后展开并缓存，后续收起再展开不重复请求。
8. 当展开内容超出终端时，按住鼠标左键拖拽节点主体或画布空白处；整个画布会随指针平移，屏幕外节点会进入可见区域。
9. 当前节点是 anchor 时可用 `dd` 取消入口；`dp` / `dn` 清除当前节点左/右分支，按 `q` 退出。普通共享节点执行 `dd` 不会删除整个连通图。
10. 快捷键记不清时按 `?` 打开完整帮助；左下 footer 只保留高频入口。

相同 hierarchy kind 和精确源码位置在整个画布中只显示一个节点，菱形关系会汇合到共享方框。通常 caller/parent 在左、callee/child 在右；循环、自环或无法保持该方向的边以黄色双线和特殊符号显示。

搜索框中的路径和行号来自当前分析 provider，内部使用从零开始的行列号保存，界面行号按从一开始显示。候选默认限定在 `--workspace` 指定的项目目录内，第三方依赖符号不会显示。

如果 `into`、`is_some` 等通用方法淹没了项目关系，可以在 workspace 根目录添加 `.cgraph.toml`。默认搜索和 hierarchy 只保留项目内符号；如需显示系统头文件或第三方依赖，可将 `[filters].workspace_only` 设为 `false`。方法会尽可能以 `Class::method` 显示，配置可用 `*::is_some` 过滤任意类的同名方法；也可以设置 `$EDITER` 后在 Canvas 输入 `ec`，编辑器返回时自动重载并刷新已加载图。完整格式见[项目配置](project-configuration.md)。

底部状态摘要描述整个分析后端，搜索弹窗中的 `Waiting for typing pause…` / `Searching workspace symbols…` 只描述当前搜索。即使 LSP 后端显示 `Working`，也可以尝试搜索；结果是否完整取决于语言服务器当时已经建立的索引。

如果 LSP 没有启动，cgraph 会尝试初始化 Rust、C、C++ 或 Python 的 Tree-sitter grammar/query，并在底部状态栏显示结果。LSP 已连接但没有声明当前 hierarchy kind 时，也会透明使用同语言 Tree-sitter 后备；例如 rust-analyzer 当前没有标准 type hierarchy，所以 Rust 类型展开走静态索引，而搜索和调用展开仍走 rust-analyzer。第一次 `ac` / `at` 搜索、纯静态展开或能力回退会惰性扫描项目源文件，之后复用同一索引。Tree-sitter 只返回能按名称唯一绑定的项目内语法关系；动态分派、同名歧义和项目外目标可能省略，展开完成后底栏会显示 `syntactic relations only` 提醒。

## 创建初始图入口

```bash
cgraph call Foo::Bar
cgraph type Student
```

这两种命令会立即创建一个尚未解析源码位置的 provisional anchor。首次 `tl` / `tr` 时，cgraph 先用 workspace symbol 将名称解析为精确源码位置；如果图中已有相同 resolved identity，入口会合并到已有节点。同名候选不唯一时会显示 `[!]`，此时应使用 `ac` / `at` 选择带精确位置的入口。

## 下一步

- 阅读[命令与交互](commands.md)了解全部当前按键。
- 如果项目没有被自动识别，阅读[语言服务器配置](language-servers.md)。
- 如果搜索一直为空或报错，阅读[故障排查](troubleshooting.md)。
- 如果希望在 cgraph 节点与 Neovim 位置之间双向跳转，阅读[编辑器联动](editor-integration.md)。
