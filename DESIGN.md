# ctree 产品设计总览

## 产品愿景

ctree 是一个基于 LSP 或 Tree-sitter 的交互式 TUI，用于发现符号并浏览函数调用层次和类型继承层次。用户应能从命令行快速进入当前项目的关系画布，在不离开终端的情况下搜索、展开、导航和管理有向关系图。

## 产品范围

主要入口为：

```bash
# 打开空画布
ctree

# 创建 call hierarchy 初始根
ctree call Foo::Bar

# 创建 type hierarchy 初始根
ctree type Student
```

近期产品范围包括：

- 当前项目的 call/type 符号发现；
- 左右双向的 call/type hierarchy 探索；
- 支持多个入口、共享节点、循环、选择和移动的关系画布；
- LSP 与未来 Tree-sitter 分析后端；
- 与编辑器联动的 Unix socket IPC；
- 不覆盖已有文件的关系图导出。

## 产品原则

- **真实状态**：未实现、不可用、正在加载和空结果必须明确区分，不能用成功状态掩盖能力缺失。
- **渐进查询**：昂贵的 hierarchy 关系按需加载，成功结果缓存；取消和错误不伪装成空结果。
- **语义节点唯一**：同一 hierarchy kind 和源码位置表示的符号在画布图中只出现一次；内部节点句柄与可补全的符号身份仍须分离，避免重载、同名符号和临时 CLI 查询被错误合并。
- **项目优先**：默认搜索当前 workspace 的项目符号，不把第三方依赖混入主要候选。
- **项目可调**：噪声符号因项目而异，应通过可版本控制的项目本地配置管理，而不是硬编码全局黑名单。
- **键鼠一致**：核心画布和弹窗行为应同时考虑键盘与鼠标，视觉布局与命中区域使用同一坐标来源。
- **安全降级**：LSP、终端尺寸或外部客户端不可用时，TUI 应保持可退出、可诊断，不破坏终端或用户文件。

## 需求导航

详细需求、父子关系、状态、优先级和验收条件统一维护在 [`requirements/`](requirements/README.md)。本文件不重复保存具体按键和边界，避免出现两个产品规范来源。

| 父需求 | 内容 |
| --- | --- |
| [REQ-1 会话与启动](requirements/REQ-1-session/README.md) | 启动模式、初始图入口和退出 |
| [REQ-2 分析后端状态](requirements/REQ-2-analysis-status/README.md) | LSP/Tree-sitter 状态和右下角窗口 |
| [REQ-3 层次关系探索](requirements/REQ-3-hierarchy/README.md) | 展开、缓存、重复节点和刷新 |
| [REQ-4 画布与导航](requirements/REQ-4-canvas-navigation/README.md) | 节点选择、空间导航和 viewport |
| [REQ-5 符号与树管理](requirements/REQ-5-symbol-management/README.md) | `ac`/`at` 搜索、新增、重定位和删除 |
| [REQ-6 进程间通信](requirements/REQ-6-ipc/README.md) | 编辑器跳转和外部查询 |
| [REQ-7 导出关系图](requirements/REQ-7-export/README.md) | 安全的文本导出 |
| [REQ-8 语言支持](requirements/REQ-8-language-support/README.md) | Rust、C/C++ 和 Python |
| [REQ-9 项目本地配置与符号过滤](requirements/REQ-9-project-configuration/README.md) | `.ctree.toml`、限定方法名和通配过滤 |

## 文档边界

| 位置 | 负责内容 | 不负责内容 |
| --- | --- | --- |
| `requirements/` | 用户可观察行为、父子关系、状态和验收条件 | 实现细节、教程 |
| `docs/` | 当前版本的安装、使用和故障排查 | 规划中功能的承诺 |
| `src/` | 架构、协议、算法取舍、代码注释和内部 TODO | 重复定义产品需求 |
| `DESIGN.md` | 愿景、范围、原则和导航 | 逐项需求状态 |

需求与实现冲突时，先确认是实现缺陷还是需求变更：实现缺陷应修正代码；需求变更应先更新 `requirements/`，再同步用户文档和内部设计。

## 技术架构入口

当前实现使用 Rust、Ratatui、Tokio、LSP 协议类型，并为未来 Tree-sitter provider 预留边界。模块职责、数据流和依赖方向见 [`src/README.md`](src/README.md)。rust-analyzer 的独立进程模型、冷索引原因和未来复用方案见 [`src/fetch/rust-analyzer.md`](src/fetch/rust-analyzer.md)。
