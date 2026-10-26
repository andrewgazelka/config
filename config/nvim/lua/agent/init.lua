-- Dispatch the current line or selection to a `claude --bg` background agent.
-- The line is tagged TODO with a spinner and flips to DONE once the session
-- stops being busy in `claude agents --json` (bg sessions have no exit code).

local M = {}

local ns = vim.api.nvim_create_namespace("claude_agent")
local SPINNER = { "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏" }
local HL = { TODO = "DiagnosticInfo", DONE = "DiagnosticOk", FAIL = "DiagnosticError" }

local tasks = {} -- in-flight, keyed by extmark id so a task follows its line
local resumable = {} -- finished sessions, keyed by extmark id
local last_resumable

local config = {
  cmd = "claude",
  permission_mode = "bypassPermissions",
  model = nil,
  poll_ms = 2500,
  poll_timeout_ms = 30 * 60 * 1000,
}

-- Strip one leading tag; matching literals, since a char class would also eat ADD, NOTE, ...
local function strip_status(line)
  for tag in pairs(HL) do
    local out, n = line:gsub("^(%s*)" .. tag .. "%s+", "%1")
    if n > 0 then return out end
  end
  return line
end

local function agents(cb)
  vim.system({ config.cmd, "agents", "--json" }, { text = true }, function(obj)
    vim.schedule(function()
      local ok, list = pcall(vim.json.decode, obj.stdout or "")
      cb(ok and type(list) == "table" and list or {})
    end)
  end)
end

local function set_status(rec, status, virt)
  if not vim.api.nvim_buf_is_valid(rec.buf) then return end
  local row = vim.api.nvim_buf_get_extmark_by_id(rec.buf, ns, rec.mark, {})[1]
  if not row then return end
  local line = vim.api.nvim_buf_get_lines(rec.buf, row, row + 1, false)[1] or ""
  local indent = line:match("^%s*")
  local new = indent .. status .. " " .. strip_status(line):gsub("^%s*", "")
  if new ~= line then vim.api.nvim_buf_set_lines(rec.buf, row, row + 1, false, { new }) end
  vim.api.nvim_buf_set_extmark(rec.buf, ns, row, 0, {
    id = rec.mark,
    virt_text = virt and { { virt, HL[status] } } or nil,
    virt_text_pos = "eol",
  })
end

local function stop_timers(rec)
  for _, key in ipairs({ "spinner", "poll" }) do
    if rec[key] then
      rec[key]:stop()
      rec[key]:close()
      rec[key] = nil
    end
  end
end

local function finish(rec, status, virt)
  stop_timers(rec)
  set_status(rec, status, virt)
  tasks[rec.mark] = nil
end

local function start_spinner(rec)
  local i = 0
  rec.spinner = vim.uv.new_timer()
  rec.spinner:start(0, 100, vim.schedule_wrap(function()
    i = i % #SPINNER + 1
    set_status(rec, "TODO", SPINNER[i] .. " " .. (rec.id or "starting"))
  end))
end

local function start_poll(rec)
  local polls, idle, elapsed = 0, 0, 0
  rec.poll = vim.uv.new_timer()
  rec.poll:start(config.poll_ms, config.poll_ms, vim.schedule_wrap(function()
    elapsed = elapsed + config.poll_ms
    if elapsed >= config.poll_timeout_ms then
      return finish(rec, "TODO", "⏱ poll timed out · " .. rec.id)
    end
    agents(function(list)
      if not tasks[rec.mark] then return end
      local found
      for _, s in ipairs(list) do
        if type(s.sessionId) == "string" and s.sessionId:sub(1, 8) == rec.id then
          found = s
          break
        end
      end
      polls = polls + 1
      local done
      if found then
        rec.full = found.sessionId
        if found.status == "busy" then
          rec.seen_busy, idle = true, 0
        else
          idle = idle + 1
        end
        -- busy -> idle, or never seen busy but idle twice (a quick task)
        done = found.status ~= "busy" and (rec.seen_busy or idle >= 2)
      else
        done = rec.seen_busy or polls >= 3 -- reaped, or never appeared
      end
      if done then
        last_resumable = { id = rec.id, full = rec.full or rec.id, dir = rec.dir }
        resumable[rec.mark] = last_resumable
        finish(rec, "DONE", "✓ " .. rec.id .. " · <leader>ar resume")
      end
    end)
  end))
end

function M.dispatch()
  local buf = vim.api.nvim_get_current_buf()
  local s, e = vim.fn.line("v"), vim.fn.line(".")
  if vim.fn.mode():match("[vV\22sS\19]") then
    vim.api.nvim_feedkeys(vim.keycode("<Esc>"), "nx", false)
  else
    s = e
  end
  if s > e then s, e = e, s end

  local lines = vim.api.nvim_buf_get_lines(buf, s - 1, e, false)
  local task = strip_status(table.concat(lines, "\n")):gsub("^%s+", "")
  if task == "" then return vim.notify("agent: empty task", vim.log.levels.WARN) end

  local name = vim.api.nvim_buf_get_name(buf)
  local dir = name ~= "" and vim.fn.isdirectory(vim.fs.dirname(name)) == 1 and vim.fs.dirname(name) or vim.fn.getcwd()
  local rec = { buf = buf, mark = vim.api.nvim_buf_set_extmark(buf, ns, s - 1, 0, {}), dir = dir }
  tasks[rec.mark] = rec
  start_spinner(rec)

  local args = { config.cmd, "--bg", "-n", task:gsub("%s+", " "):sub(1, 50), "--permission-mode", config.permission_mode }
  if config.model then vim.list_extend(args, { "--model", config.model }) end
  table.insert(args, "Complete this task. Be autonomous and finish it end to end.\n\nTask:\n" .. task)

  vim.system(args, { cwd = dir, text = true }, function(obj)
    vim.schedule(function()
      if not tasks[rec.mark] then return end
      rec.id = (obj.stdout or ""):match("%x%x%x%x%x%x%x%x")
      if obj.code ~= 0 or not rec.id then
        finish(rec, "FAIL", "✗ launch failed (" .. obj.code .. ")")
        return vim.notify("agent: launch failed\n" .. (obj.stderr or ""), vim.log.levels.ERROR)
      end
      start_poll(rec)
    end)
  end)
end

function M.agent_view()
  vim.cmd("botright split | terminal " .. config.cmd .. " agents")
  vim.cmd.startinsert()
end

-- `claude --resume` refuses a session still alive as a bg agent, and bg agents
-- linger after finishing, so stop an idle one first. A busy one is left alone.
function M.resume()
  local row = vim.fn.line(".") - 1
  local target
  for _, m in ipairs(vim.api.nvim_buf_get_extmarks(0, ns, { row, 0 }, { row, -1 }, {})) do
    local t = resumable[m[1]] or (tasks[m[1]] and tasks[m[1]].full and tasks[m[1]])
    if t then
      target = { full = t.full, dir = t.dir }
      break
    end
  end
  target = target or last_resumable
  if not target then return vim.notify("agent: no resumable session here", vim.log.levels.WARN) end

  local function open()
    vim.cmd("botright new")
    vim.fn.jobstart({ config.cmd, "--resume", target.full }, { term = true, cwd = target.dir })
    vim.cmd.startinsert()
  end
  agents(function(list)
    for _, s in ipairs(list) do
      if s.sessionId == target.full then
        if s.status == "busy" then
          vim.notify("agent: still running, opening agent view", vim.log.levels.WARN)
          return M.agent_view()
        elseif s.pid then
          pcall(vim.uv.kill, s.pid, "sigterm")
          return vim.defer_fn(open, 800) -- let it release the session
        end
      end
    end
    open()
  end)
end

function M.setup(opts)
  config = vim.tbl_extend("force", config, opts or {})
  vim.api.nvim_create_autocmd({ "BufWipeout", "BufDelete" }, {
    callback = function(ev)
      for mark, rec in pairs(tasks) do
        if rec.buf == ev.buf then
          stop_timers(rec)
          tasks[mark] = nil
        end
      end
    end,
  })
  vim.keymap.set({ "n", "x" }, "<leader>aa", M.dispatch, { desc = "Agent: dispatch line/selection" })
  vim.keymap.set("n", "<leader>av", M.agent_view, { desc = "Agent: `claude agents` view" })
  vim.keymap.set("n", "<leader>ar", M.resume, { desc = "Agent: resume session on this line" })
end

return M
