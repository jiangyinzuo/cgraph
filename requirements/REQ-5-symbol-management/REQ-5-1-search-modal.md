# REQ-5-1：ac/at 搜索弹窗

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-5` |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 需求

Canvas 模式输入 `ac` 打开 call symbol 搜索，输入 `at` 打开 type symbol 搜索。弹窗提供 LSP Query、Symbol 和 URI 三个独立输入框，位于屏幕中央，并支持键盘和鼠标选择。

## 验收条件

- 普通字符和 Unicode Backspace 编辑当前输入框。
- `Tab` 按 LSP Query → Symbol → URI → LSP Query 的顺序循环焦点，不把结果列表加入焦点循环。
- 不增加其他输入框跳转快捷键；活动输入框必须有不依赖颜色的标签和清晰的边框高亮。
- 三个纵向输入框共享相邻横边界，不在两个字段之间重复绘制两行边框；活动框后绘制，保证共享边仍使用活动样式。
- `Up`/`Down`、`Ctrl-p`/`Ctrl-n`、鼠标移动和滚轮改变选择。
- `Enter` 或鼠标左键接受结果。
- `Esc` 关闭弹窗且不创建节点。
- call/type 结果使用各自允许的符号种类。
