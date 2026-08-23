# 编辑器联动

cgraph 通过同一个 Unix socket 与 Neovim 等编辑器双向联动：用户双击节点时，cgraph 广播源码位置；编辑器也可以请求 cgraph 聚焦或创建 call/type anchor。

## 启动 socket

调用方负责创建私有 runtime 目录，并为每个同时运行的 cgraph 实例选择不同路径：

```bash
install -d -m 700 "$XDG_RUNTIME_DIR/cgraph"
cgraph --workspace /path/to/project \
  --ipc-socket "$XDG_RUNTIME_DIR/cgraph/project.sock"
```

cgraph 不自动创建父目录；父目录必须是真实目录而非符号链接，并且不能允许 group/other 写入。cgraph 也不会覆盖普通文件、符号链接或无法确认归属的 socket，同一路径已有活跃实例时会拒绝启动。成功 bind 后 socket 和 ownership marker 权限均为 `0600`。正常退出会清理本实例文件；崩溃后重启时，只有 marker 记录的 device/inode 与 socket 一致且连接明确被拒绝，才会自动回收陈旧 socket。

## 消息格式

传输使用 newline-delimited JSON：每个物理行都是一个完整 envelope，字符串中的换行由 JSON 转义。cgraph 接受的单个入站帧（包含结尾换行）不能超过 1 MiB。

双击节点产生无 request id 的广播事件：

```json
{"version":1,"request_id":null,"payload":{"type":"open_location","uri":"file:///project/src/main.rs","line":8,"character":3}}
```

- `version` 当前必须是 `1`；客户端遇到其他版本应拒绝处理。
- `request_id` 对广播事件为 `null`；客户端请求必须使用非空无符号整数。
- `line` 是从零开始的行号；Neovim 光标 API 使用从一开始的行号，因此需要加一。
- `character` 是从零开始的 UTF-16 code-unit 偏移，不是字节偏移。cgraph 对 LSP 只协商 UTF-16，并把 Tree-sitter 的 UTF-8 字节列转换到同一坐标系。
- 事件广播给当时所有已连接客户端。某个客户端断开或持续不读取时会被独立移除，不阻塞 TUI 和其他客户端。

## 聚焦符号请求

编辑器可以发送 `focus_symbol`。`hierarchy` 必须是 `call` 或 `type`，`symbol` 不能为空；`location` 可以是 `null`：

```json
{"version":1,"request_id":42,"payload":{"type":"focus_symbol","hierarchy":"call","symbol":"main","location":{"uri":"file:///project/src/main.rs","line":7,"character":2}}}
```

有位置时，`uri` 必须是非空 `file://` URI，line 和 UTF-16 character 必须同时存在且从零开始。cgraph 使用与本地搜索相同的 hierarchy kind + 精确位置身份，因此已有语义节点会被复用。没有位置时可以发送：

```json
{"version":1,"request_id":43,"payload":{"type":"focus_symbol","hierarchy":"type","symbol":"Worker","location":null}}
```

无位置请求会复用同 kind 的唯一同名节点；没有候选时创建 provisional anchor；多个候选时返回 ambiguous 错误，客户端应补充精确位置。成功后，cgraph 固定、选中并居中节点，然后返回：

```json
{"version":1,"request_id":42,"payload":{"type":"accepted"}}
```

`accepted` 只表示 App 已经完成 anchor 状态迁移，不表示左右 hierarchy 已经加载。需要关系时仍由用户执行 `tl` / `tr`。验证、版本或语义错误返回：

```json
{"version":1,"request_id":43,"payload":{"type":"error","message":"symbol \"Worker\" is ambiguous; include an exact source location"}}
```

能够识别 request id 的错误会原样带回该 id；帧在解析 envelope 前就无效时，响应 id 为 `null`。版本不为 `1`、请求没有 id、JSON/payload 不合法或帧超过 1 MiB 时，cgraph 不会猜测客户端意图。客户端应始终按 request id 关联响应，不依赖多个请求的返回顺序。

## Neovim Lua 示例

以下最小模块同时接收 `open_location` 并发送 `focus_symbol`。保存为 `lua/cgraph_ipc.lua`：

```lua
local M = {}
local next_request_id = 1
local pending_requests = {}

local function notify(message, level)
  vim.schedule(function()
    vim.notify(message, level or vim.log.levels.INFO)
  end)
end

local function open_location(payload)
  if payload.type ~= "open_location" then
    return
  end

  local filename = vim.uri_to_fname(payload.uri)
  vim.cmd.edit(vim.fn.fnameescape(filename))
  local text = vim.api.nvim_buf_get_lines(0, payload.line, payload.line + 1, false)[1] or ""
  local ok, byte_column = pcall(
    vim.str_byteindex,
    text,
    "utf-16",
    payload.character,
    false
  )
  if not ok then
    byte_column = #text
  end
  vim.api.nvim_win_set_cursor(0, { payload.line + 1, byte_column })
end

local function handle_message(message)
  if message.version ~= 1 then
    notify("cgraph IPC version mismatch", vim.log.levels.ERROR)
    return
  end

  if message.request_id == vim.NIL or message.request_id == nil then
    vim.schedule(function()
      open_location(message.payload)
    end)
    return
  end

  local request = pending_requests[message.request_id]
  pending_requests[message.request_id] = nil
  if not request then
    notify("unexpected cgraph IPC response " .. message.request_id, vim.log.levels.WARN)
    return
  end

  if message.payload.type == "error" then
    notify(
      string.format("cgraph rejected %s %q: %s", request.hierarchy, request.symbol, message.payload.message),
      vim.log.levels.ERROR
    )
  elseif message.payload.type ~= "accepted" then
    notify("unknown cgraph IPC response", vim.log.levels.ERROR)
  end
end

function M.connect(path)
  local uv = vim.uv or vim.loop
  local pipe = uv.new_pipe(false)
  local pending = ""
  M.pipe = pipe
  M.connected = false

  pipe:connect(path, function(error_message)
    if error_message then
      notify("cgraph IPC connect failed: " .. error_message, vim.log.levels.ERROR)
      return
    end
    M.connected = true

    pipe:read_start(function(read_error, chunk)
      if read_error then
        notify("cgraph IPC read failed: " .. read_error, vim.log.levels.ERROR)
        return
      end
      if not chunk then
        M.connected = false
        return
      end

      pending = pending .. chunk
      while true do
        local newline = pending:find("\n", 1, true)
        if not newline then
          break
        end
        local frame = pending:sub(1, newline - 1)
        pending = pending:sub(newline + 1)
        local ok, message = pcall(vim.json.decode, frame)
        if ok then
          handle_message(message)
        else
          notify("invalid cgraph IPC JSON", vim.log.levels.ERROR)
        end
      end
    end)
  end)
end

function M.focus(hierarchy, symbol, location)
  if not M.connected then
    notify("cgraph IPC is not connected", vim.log.levels.ERROR)
    return nil
  end
  if hierarchy ~= "call" and hierarchy ~= "type" then
    error("cgraph hierarchy must be 'call' or 'type'")
  end

  local request_id = next_request_id
  next_request_id = next_request_id + 1
  pending_requests[request_id] = { hierarchy = hierarchy, symbol = symbol }

  local envelope = {
    version = 1,
    request_id = request_id,
    payload = {
      type = "focus_symbol",
      hierarchy = hierarchy,
      symbol = symbol,
      location = location or vim.NIL,
    },
  }
  M.pipe:write(vim.json.encode(envelope) .. "\n", function(write_error)
    if write_error then
      pending_requests[request_id] = nil
      notify("cgraph IPC write failed: " .. write_error, vim.log.levels.ERROR)
    end
  end)
  return request_id
end

function M.current_location()
  local filename = vim.api.nvim_buf_get_name(0)
  if filename == "" then
    return nil
  end
  local cursor = vim.api.nvim_win_get_cursor(0)
  local line = vim.api.nvim_get_current_line()
  local character = vim.str_utfindex(line, "utf-16", cursor[2], false)
  return {
    uri = vim.uri_from_fname(filename),
    line = cursor[1] - 1,
    character = character,
  }
end

return M
```

在 Neovim 中使用与 cgraph 相同的 socket 路径：

```lua
require("cgraph_ipc").connect(vim.env.XDG_RUNTIME_DIR .. "/cgraph/project.sock")
```

连接成功后，可以把当前光标位置作为精确 call anchor 发送给 cgraph：

```lua
local cgraph = require("cgraph_ipc")
cgraph.focus("call", "main", cgraph.current_location())
```

如果编辑器只有符号名，也可以传 `nil`；当画布存在多个同名节点时，cgraph 会返回可诊断的 ambiguous 错误：

```lua
require("cgraph_ipc").focus("type", "Worker", nil)
```

客户端应把 socket 路径视为可信本地配置，不应把事件 URI 拼接成 shell 命令。示例使用 `vim.uri_to_fname`、`fnameescape` 和 Neovim API 直接打开文件，并使用编码感知的 `vim.str_utfindex` / `vim.str_byteindex` 在 Neovim byte column 与协议 UTF-16 character 之间转换。若使用的 Neovim 版本没有这些带 encoding 参数的 API，应先升级或在适配器中实现等价转换，不能把 UTF-8 字节列直接发送给 cgraph。
