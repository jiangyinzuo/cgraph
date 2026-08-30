# REQ-5-3：候选过滤与排序

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-5` |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 需求

server 返回的 workspace symbols 应只删除协议字段完全相同的重复项，并在客户端执行不区分大小写的模糊筛选；不能按 symbol name 合并不同 URI 或 range 的同名定义。Symbol 输入框只匹配语言适配后的完整 symbol display name；URI 输入框只匹配候选的 URI/显示路径。每个本地输入框都作为一个普通模糊子序列处理，空白仅用于分隔和提高可读性，匹配前忽略，不引入 AND、OR 或其他布尔查询语义。默认只展示当前 workspace 中的项目符号，不展示依赖或其他工作区外文件。

## 验收条件

- call 搜索接受 function、method、constructor。
- type 搜索接受 class、interface、struct、enum、type parameter。
- 使用 `nucleo-matcher` 执行 Unicode-aware 的模糊匹配与评分；精确、前缀和更紧凑的子序列匹配优先。
- Symbol 与 URI 条件同时存在时，候选必须分别通过两个字段的普通模糊匹配；单个字段内不拆分为多个布尔条件。
- 名称、kind、URI、range 和 container 均相同的协议级重复项只显示一次；同名但位置不同的项全部保留。
- 非 file URI 和 canonical workspace root 外的结果默认排除。
- 项目本地的额外符号过滤由 [REQ-9](../REQ-9-project-configuration/README.md) 定义，并在本地模糊排序前应用。
- LSP 可用时不聚合 Tree-sitter workspace-symbol 候选；C/C++ 搜索保持 clangd 的标准响应语义。

## 当前边界

cgraph 仍遵守 server 的单次结果数量上限；本地排序不会枚举 server 未返回的全部索引。clangd 等 server 若已按自身 SymbolID/USR 合并同名定义，客户端无法通过标准 LSP 恢复被省略的位置。
