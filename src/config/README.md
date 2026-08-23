# 项目本地配置设计

相关产品规范：[REQ-9 项目本地配置与符号过滤](../../requirements/REQ-9-project-configuration/README.md)。

`config` 模块只负责定位 workspace 根目录的 `.cgraph.toml`、安全创建最小模板、校验 schema，并生成 UI 与 provider 无关的配置值。启动和 `ec` 返回后都走同一个严格 loader；本模块不监控文件变化、不启动编辑器，也不直接刷新图。

## 当前 schema

```toml
[filters]
symbols = ["*::into", "Option::is_some", "*::Some"]
```

`filters.symbols` 是针对完整显示名的大小写敏感模式集合，匹配覆盖整个字符串；`*` 是唯一的通配符，表示任意数量字符。面向对象方法在 provider 给出容器信息时先规范化为 `Class::method`，再执行匹配，因此 `*::is_some` 能过滤任意类的 `is_some`，而 `Option::is_some` 只过滤指定类。普通函数仍使用自身名称。

加载时会去掉每项首尾空白并按首次出现顺序合并重复模式；空字符串、未知字段和错误类型都会使启动失败，并在错误中包含配置文件路径。不存在 `.cgraph.toml` 等同于空配置。通配符匹配使用动态规划而不是回溯，避免多个 `*` 对长限定名造成指数级耗时。

过滤发生在 App 接收已经归一化的查询结果之后，而不是 LSP 或 Tree-sitter 适配器中。这样 workspace symbol 和两种 hierarchy provider 共享同一规则，provider 仍能保留完整响应供其他用途。用户显式通过 CLI 创建的 anchor 不受过滤：规则只减少可发现候选和新加载的邻接节点，不应让一个明确请求悄悄消失。

`ProjectConfig::create_if_missing` 使用 `create_new` 写入最小有效模板。文件在检查与创建之间由其他进程生成时，只接受 `AlreadyExists`，绝不截断竞态中的用户内容。TUI 选择编辑器、管理终端和处理退出状态；App 替换过滤器后为所有已加载或正在刷新的可达分支生成新 request id。这个分工保证无效重载能保留旧配置，也保证编辑期间完成的旧查询不能覆盖新规则结果。

## 后续扩展

- 支持按 hierarchy kind、容器或源码路径缩小规则范围。
- 在确有需求时增加 `?`、字符组或显式 regex；当前故意只支持可预测的 `*`。
- 如果未来需要自动监控文件，复用同一严格 loader 和全图 refresh 边界，并对连续保存进行防抖。
