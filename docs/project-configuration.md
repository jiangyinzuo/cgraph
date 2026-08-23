# 项目配置

ctree 会在启动时读取 `--workspace` 根目录中的 `.ctree.toml`。该文件适合提交到项目版本库，使团队成员看到一致的关系图；没有这个文件时不启用任何符号过滤。

## 过滤符号

```toml
[filters]
symbols = [
  "*::into",
  "Option::is_some",
  "*::Some",
]
```

规则针对界面中的完整显示名，并且必须匹配整个名称：

- 普通函数使用 server 返回的名称，例如 `main`。
- 方法和构造器在 server 提供类或容器信息时显示为 `Class::method`，不同语言统一使用 `::` 作为界面分隔符。
- `*` 匹配零个或多个字符，包括 `::`；这是当前唯一的通配符。
- 匹配区分大小写，所以 `*::Some` 不会过滤 `Option::some`。
- 没有通配符的规则是整串精确匹配；`Option::is_some` 不会过滤 `Result::is_some`。

过滤同时作用于 `ac` / `at` 搜索候选和 `tl` / `tr` 新加载的邻接节点。它不会删除已经显式写在命令行中的 anchor，例如 `ctree call Option::is_some` 仍会创建该入口。

## 加载与错误

配置只在启动时读取一次。修改 `.ctree.toml` 后需要重新启动 ctree；这样同一会话不会混用两套规则与 hierarchy 缓存。

无效 TOML、未知字段、错误的数据类型和空字符串模式都会阻止启动，错误消息包含 `.ctree.toml` 的路径。ctree 不会静默忽略拼错的字段，因为那会让用户误以为过滤已经生效。

call hierarchy 协议不强制语言服务器提供类名。如果 server 没有返回可用的容器信息，ctree 会保留原始方法名；此时只能用该实际显示名编写规则。
