# REQ-5-2：异步查询生命周期

| 字段 | 值 |
| --- | --- |
| 父需求 | `REQ-5` |
| 状态 | `Implemented` |
| 优先级 | `P0` |
| 目标版本 | `0.1` |

## 需求

搜索行为参考 VS Code workspace symbol quick access：打开弹窗或输入变化后等待约 200 ms，再把当前完整 query 异步发送给 provider。查询没有两个字符的最小长度，空输入也允许发送。

## 验收条件

- 连续输入重置防抖，只发送稳定后的完整 query。
- 防抖阶段显示 `Waiting for typing pause…`，请求真正开始后才显示 `Searching workspace symbols…`。
- 已发出的旧 LSP 请求使用 `$/cancelRequest` 取消。
- request id 拒绝取消后仍返回的迟到结果。
- 关闭弹窗取消待发或进行中的查询任务。
