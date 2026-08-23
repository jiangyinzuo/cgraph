# 导出关系图

在 Canvas 模式按 `w` 打开 `Save graph` 弹窗，输入一个尚不存在的目标路径并按 `Enter`。保存成功后弹窗关闭，底栏显示实际目标路径；按 `Esc` 可以取消。

目标文件已经存在时，cgraph 会拒绝写入，原内容保持不变。空路径、父目录不存在、权限不足或目标是目录时，错误会显示在弹窗中，修改路径后可以直接重试。相对路径按 cgraph 进程的当前工作目录解析，不会自动改成 `--workspace` 路径。

## 导出范围

文件包含从当前 anchors 可达的全部已知关系，包括已经查询并缓存、但此刻在画布上收起的分支。viewport 外或被屏幕裁剪的节点照常导出；取消 anchor 或清除分支后不再从任何 anchor 可达的孤立节点不会导出。

## 文本格式

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

`Nodes` 为每个节点分配文件内局部编号，并注明 call/type、名称、可选的 anchor 标记和源码位置。用户可见的行列从 1 开始；位置未知时直接写 `location unknown`。

`Relations` 使用箭头表示方向，同时重复两端名称，阅读单条关系时不必频繁回查节点表。共享节点只定义一次，多条关系引用同一个编号；循环和自环通过箭头指回已有编号自然表达。相同图状态的输出顺序稳定，适合版本比较。
