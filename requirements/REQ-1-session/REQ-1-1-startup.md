# REQ-1-1：启动与初始图入口

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-1` |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 需求

cgraph 必须支持以下入口：

```text
cgraph
cgraph call <SYMBOL>
cgraph type <SYMBOL>
```

无子命令时打开空画布；call/type 子命令分别创建对应种类的初始 anchor。工作区默认是当前目录，并允许通过 `--workspace` 覆盖。

## 验收条件

- 三种命令均可被 CLI 解析。
- 空画布明确显示为空，不制造虚假节点。
- 初始 anchor 显示用户输入的名称和 call/type 类型。

## 实现证据

- CLI 解析测试位于 `src/cli.rs`。
- anchor 初始化状态位于 `src/app.rs`。
