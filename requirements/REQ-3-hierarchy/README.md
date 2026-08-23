# REQ-3：层次关系探索

| 字段 | 值 |
| --- | --- |
| 状态 | `Implemented` |
| 优先级 | `P1` |
| 目标版本 | `0.1` |

## 目标

用户能够围绕 call 或 type 入口节点向左右探索关系，并在收起、重新展开和刷新后保持可预测的有向图状态。

左侧表示调用者或父类型，右侧表示被调用者或子类型。数据结构可以把两侧都建模为节点的分支，但 UI 必须保留方向语义。

## 子需求

| 子需求 | 状态 | 摘要 |
| --- | --- | --- |
| [REQ-3-1 双向展开与收起](REQ-3-1-expand-collapse.md) | `Implemented` | 左右按钮及 `tl` / `tr` 操作 |
| [REQ-3-2 懒加载与缓存](REQ-3-2-lazy-cache.md) | `Implemented` | 首次展开查询并保留深层状态 |
| [REQ-3-3 全局语义节点去重](REQ-3-3-duplicates.md) | `Implemented` | 相同符号只显示一次并保留所有关系 |
| [REQ-3-4 单层刷新](REQ-3-4-refresh.md) | `Implemented` | 同时刷新唯一语义节点的左右一层孩子 |

## 父需求验收

- call 和 type 都能准备精确根符号并执行双向单层查询。
- 展开、收起、缓存、全局节点去重和刷新符合各子需求不变量。
- 查询错误、取消和“不支持”不伪装成成功的空分支。

## 当前实现与差距

LSP call/type prepare、双向单层请求、独立异步加载、分支缓存、失败重试、全局语义节点去重、循环安全可见图和显式双向单层刷新已经连接。没有 LSP 时，Rust、C、C++ 和 Python 共用一个惰性 Tree-sitter 项目索引，向同一 `HierarchyQuery` / `HierarchyResponse` 边界提供项目内静态调用和类型关系。

Tree-sitter 只能返回语法上可唯一绑定的关系：动态分派、同名歧义和项目外目标可能被省略。每次 Tree-sitter hierarchy 成功后，footer notice 会明确显示 `syntactic relations only`，不会把成功空结果宣称为完整语义结论。
