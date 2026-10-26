local au = vim.api.nvim_create_autocmd

-- Reload buffers the moment the file changes on disk; 'autoread' alone
-- only checks on focus/shell events.
local watchers = {}
au("BufEnter", {
  callback = function(ev)
    local buf, path = ev.buf, vim.api.nvim_buf_get_name(ev.buf)
    if path == "" or watchers[buf] then return end
    local handle = vim.uv.new_fs_event()
    if not handle then return end
    watchers[buf] = handle
    handle:start(path, {}, vim.schedule_wrap(function(err)
      if err then return end
      if not vim.api.nvim_buf_is_valid(buf) then
        handle:stop()
        watchers[buf] = nil
      elseif not vim.bo[buf].modified then
        vim.api.nvim_buf_call(buf, function() vim.cmd.checktime() end)
      end
    end))
  end,
})
au("BufDelete", {
  callback = function(ev)
    if watchers[ev.buf] then
      watchers[ev.buf]:stop()
      watchers[ev.buf] = nil
    end
  end,
})

au("BufWritePre", {
  desc = "Create missing parent directories",
  callback = function()
    if vim.bo.buftype ~= "" then return end
    local dir = vim.fn.expand("%:p:h")
    if dir ~= "" and vim.fn.isdirectory(dir) == 0 then vim.fn.mkdir(dir, "p") end
  end,
})

au("BufReadPost", {
  desc = "Restore last cursor position",
  callback = function()
    -- runs before filetype detection, so match the name, not 'filetype'
    if vim.fn.expand("%:t") == "COMMIT_EDITMSG" then return end
    local line = vim.fn.line("'\"")
    if line > 1 and line <= vim.fn.line("$") then vim.cmd('normal! g`"') end
  end,
})

au("FileType", {
  pattern = "markdown",
  callback = function()
    vim.opt_local.number = false
    vim.opt_local.relativenumber = false
    local map = function(mode, lhs, rhs) vim.keymap.set(mode, lhs, rhs, { buffer = true }) end
    map("v", "<D-i>", 'c*<C-r>"*<Esc>')
    map("n", "<D-i>", "viw<Esc>a*<Esc>bi*<Esc>")
    map("i", "<D-i>", "**<Left>")
    map("v", "<D-b>", 'c**<C-r>"**<Esc>')
    map("n", "<D-b>", "viw<Esc>a**<Esc>2bi**<Esc>")
    map("i", "<D-b>", "****<Left><Left>")
  end,
})
