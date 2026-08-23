# REQ-5-1：ac/at 搜索弹窗

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-5` |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 需求

Canvas 模式输入 `ac` 打开 call symbol 搜索，输入 `at` 打开 type symbol 搜索。弹窗位于屏幕上方中央，并支持键盘和鼠标选择。

## 验收条件

- 普通字符和 Unicode Backspace 编辑查询。
- `Up`/`Down`、`Ctrl-p`/`Ctrl-n`、鼠标移动和滚轮改变选择。
- `Enter` 或鼠标左键接受结果。
- `Esc` 关闭弹窗且不创建节点。
- call/type 结果使用各自允许的符号种类。
