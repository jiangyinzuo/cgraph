# 快速上手

## 环境要求

- Rust 1.88 或更新版本。
- 一个支持标准输入输出通信的语言服务器。
- 终端需要支持 raw mode、备用屏幕和基本鼠标事件。

当前常用组合：

| 工作区 | 推荐语言服务器 |
| --- | --- |
| Rust | `rust-analyzer` |
| C/C++ | `clangd` |
| Python | `pylsp` |

语言服务器不随 ctree 一起安装，请使用系统包管理器、Rust 工具链或对应语言的包管理器安装。

## 从源码运行

```bash
git clone <ctree-repository>
cd ctree
cargo run
```

也可以安装当前工作区中的二进制：

```bash
cargo install --path .
ctree
```

## 第一次搜索

在一个 Rust 项目目录中运行：

```bash
ctree --workspace .
```

如果目录中存在 `Cargo.toml`，ctree 会尝试启动 `rust-analyzer`。进入 TUI 后：

- 最底栏右侧显示当前分析后端。`LSP: rust-analyzer · Ready` 表示连接已建立；`Working` 及任务消息表示语言服务器仍在加载项目、索引或执行其他后台工作，左侧同时保留快捷键提示。

1. 输入 `at` 打开 type 搜索框。
2. 直接输入任意长度的文本，例如 `V` 或 `Ve`；不要求先输入两个字符。
3. 停止输入约 200 ms 后，ctree 会把当前完整文本交给语言服务器。继续输入会取消过时查询并重新计时。
4. 防抖期间会显示 `Waiting for typing pause…`；请求真正发出后变为 `Searching workspace symbols…`，完成后显示结果数量。返回结果还会在本地进行不区分大小写的模糊排序。
5. 使用上下方向键选择结果，再按回车把该类型加入画布。
6. 重复搜索其他符号时，多个根会同时显示；重新选择已有语义根会把它移回中心而不重复创建。
7. 使用方向键或 `h`、`j`、`k`、`l` 在可见节点间移动，也可以鼠标单击节点；首次按 `tl` / `tr` 会异步加载对应方向的一层关系，成功后展开并缓存，后续收起再展开不重复请求。
8. 当展开内容超出终端时，按住鼠标左键拖拽节点主体或画布空白处；整个画布会随指针平移，屏幕外节点会进入可见区域。
9. 使用 `dd` 删除当前树，`dp` / `dn` 删除当前节点左/右分支，按 `q` 退出。

搜索框中的路径和行号来自语言服务器，内部使用从零开始的 LSP 行列号保存，界面行号按从一开始显示。候选默认限定在 `--workspace` 指定的项目目录内，第三方依赖符号不会显示。

如果 `into`、`is_some` 等通用方法淹没了项目关系，可以在 workspace 根目录添加 `.ctree.toml`。方法会尽可能以 `Class::method` 显示，配置可用 `*::is_some` 过滤任意类的同名方法；完整格式见[项目配置](project-configuration.md)。

底部状态摘要描述整个分析后端，搜索弹窗中的 `Waiting for typing pause…` / `Searching workspace symbols…` 只描述当前搜索。即使后端显示 `Working`，也可以尝试搜索；结果是否完整取决于语言服务器当时已经建立的索引。

如果 LSP 没有启动，ctree 会尝试初始化 Rust、C、C++ 或 Python 的 Tree-sitter grammar/query，并在底部状态栏显示结果。这个 fallback 当前表示语法 parser 已准备好，不提供 `ac` / `at` workspace symbol 搜索；搜索仍需配置 LSP。

## 创建初始根节点

```bash
ctree call Foo::Bar
ctree type Student
```

这两种命令会立即创建一个尚未解析源码位置的根节点。首次 `tl` / `tr` 时，ctree 先用 workspace symbol 将名称解析为精确源码位置；同名候选不唯一时会显示 `[!]`，此时应使用 `ac` / `at` 选择带精确位置的根。

## 下一步

- 阅读[命令与交互](commands.md)了解全部当前按键。
- 如果项目没有被自动识别，阅读[语言服务器配置](language-servers.md)。
- 如果搜索一直为空或报错，阅读[故障排查](troubleshooting.md)。
