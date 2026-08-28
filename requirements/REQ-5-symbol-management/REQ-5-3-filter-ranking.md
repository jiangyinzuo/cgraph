# REQ-5-3：候选过滤与排序

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-5` |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 需求

server 返回的 workspace symbols 应去重，并在客户端执行不区分大小写的模糊筛选。第一项查询匹配 symbol name；存在后续项时，后续项匹配 symbol name、container name 与文件路径组成的元数据文本。默认只展示当前 workspace 中的项目符号，不展示依赖或其他工作区外文件。

## 验收条件

- call 搜索接受 function、method、constructor。
- type 搜索接受 class、interface、struct、enum、type parameter。
- 使用 `nucleo-matcher` 执行 Unicode-aware 的模糊匹配与评分；精确、前缀和更紧凑的子序列匹配优先。
- 相同符号不会重复显示。
- 非 file URI 和 canonical workspace root 外的结果默认排除。
- 项目本地的额外符号过滤由 [REQ-9](../REQ-9-project-configuration/README.md) 定义，并在本地模糊排序前应用。

## 当前边界

cgraph 仍遵守 server 的单次结果数量上限；本地排序不会枚举 server 未返回的全部索引。
