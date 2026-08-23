# 文本导出内部设计

本模块实现 [REQ-7 导出关系图](../../requirements/REQ-7-export/README.md) 的 UI 无关部分：从领域图生成稳定文本，并使用原子的“不存在才创建”语义写入目标文件。保存弹窗和按键属于 `tui/`，不能反向进入本模块。

## 导出边界

导出对象是从当前 anchors 出发可达的全部**已知关系**。`expanded` 只控制画布显示，不会让已经查询并缓存的关系从文件中消失；已经取消 anchor、清除分支且不再从任何 anchor 可达的存储节点不会导出。这个边界由 `RelationGraph::known_graph` 提供，与只沿展开分支遍历的 `visible_graph` 明确分离。

## 人类可读的 `text v1`

格式由一行版本头、节点区和边区组成：

```text
cgraph graph · text v1

Nodes (2)
  [1] call  main  [anchor]
      file:///project/src/main.rs:1:1
  [2] call  run
      location unknown

Relations (1)
  [1] main  →  [2] run
```

节点先按 hierarchy kind、源码 URI、行、列、名称和最终 `NodeId` 排序，再分配文件局部编号。节点主行只保留编号、call/type、名称和可选 `[anchor]`，下一行显示适合用户阅读的从一开始的源码行列；缺少位置时写成 `location unknown`，不显示 `null`。关系行重复两端编号和名称，用户无需在阅读每条边时不断回查节点表。共享节点只定义一次，多条关系引用同一编号，循环和自环则自然显示为指回较早编号的箭头。

格式刻意不使用 `kind=...`、JSON 对象或带标签的内部字段，以减少语法噪音。名称和 URI 中罕见的换行、制表符、回车及反斜杠只做最小单行转义；普通代码符号保持原样。

`NodeId` 只在所有语义字段完全相同时作为最终 tie-breaker，不直接输出。对于同一个内存图状态，序列化结果必须逐字节稳定。未来格式变化必须使用新版本头，不能静默改变 `text v1` 的含义。

## 写入安全

`write_text` 使用 `OpenOptions::create_new(true)`。检查“文件不存在”后再普通创建存在 TOCTOU 覆盖窗口，而 `create_new` 由操作系统保证目标已存在时失败，并且从不以 truncate 模式打开它。空路径、父目录不存在、权限错误、目录目标和并发创建都会返回错误，由保存弹窗显示，TUI 保持运行。
