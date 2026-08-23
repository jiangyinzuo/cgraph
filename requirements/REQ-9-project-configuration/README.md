# REQ-9：项目本地配置与符号过滤

| 字段 | 值 |
| --- | --- |
| 状态 | `Implemented` |
| 优先级 | `P1` |
| 目标版本 | `0.1` |

## 目标

每个项目可以版本控制自己的 ctree 规则，隐藏 `into`、`is_some`、`Some` 等在关系图中噪声较高、但不适合作为所有项目全局默认值的符号。

## 需求

ctree 启动时从 `--workspace` 根目录读取 `.ctree.toml`。`filters.symbols` 按完整显示名过滤 workspace symbol 搜索候选和新加载的 hierarchy 子节点；模式大小写敏感，`*` 匹配任意数量字符。

面向对象方法在语言服务器提供容器信息时统一显示为 `Class::method`，搜索结果与画布节点使用同一名称。过滤规则也以该限定名为准，例如 `Option::is_some` 只匹配一个类，`*::is_some` 匹配任意类。

## 验收条件

- 缺少 `.ctree.toml` 时保持无过滤的兼容行为。
- 无效 TOML、未知字段、错误类型和空模式在进入 TUI 前给出包含文件路径的错误。
- `*` 可以匹配零个或多个 Unicode 字符，其他字符按大小写精确匹配，模式必须覆盖完整显示名。
- 搜索候选与 incoming/outgoing hierarchy 子节点使用相同过滤器。
- 显式 CLI 根节点不被静默删除；配置只影响发现结果和后续加载的孩子。
- 限定方法名对应的节点宽度随文本扩展；终端足够宽时不截断类名或方法名。
- 配置在一次会话中保持不变，修改文件后重新启动 ctree 生效。

## 当前实现

`config` 模块使用严格 serde/TOML schema 加载项目文件，把模式编译为无回溯的通配匹配器。workspace symbol 的 `containerName` 和 call hierarchy 的可用 `detail` 会规范化为 `Container::method`；App 在缓存搜索候选或 hierarchy 孩子前统一过滤。类型 hierarchy 节点本身是类型名，不额外添加方法限定。

语言服务器协议不保证 call hierarchy 一定提供可用容器信息；缺失或明显是路径/签名的 `detail` 不会被误当成类名，此时保留 server 返回的原始方法名。

## 实现证据

- `.ctree.toml` 提供本项目的实际配置示例。
- `src/config/mod.rs` 覆盖缺省加载、模式规范化、Unicode 通配符和错误校验。
- `src/fetch/lsp.rs` 与 `src/tui/mod.rs` 覆盖搜索及 hierarchy 的限定方法名。
- `src/app.rs` 的状态机测试同时验证搜索和 hierarchy 过滤，并确认大小写与整串匹配边界。
