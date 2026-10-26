vim.g.mapleader = " "

-- Long history: marks for 1000 files, 1000 commands, 100 searches.
vim.o.shada = "'1000,<50,s10,h,/100,:1000"
vim.o.undofile = true
vim.o.swapfile = false

-- OSC 52 so yanks reach the local clipboard over SSH too.
local osc52 = require("vim.ui.clipboard.osc52")
vim.g.clipboard = {
  name = "OSC 52",
  copy = { ["+"] = osc52.copy("+"), ["*"] = osc52.copy("*") },
  paste = { ["+"] = osc52.paste("+"), ["*"] = osc52.paste("*") },
}
vim.o.clipboard = "unnamedplus"

vim.o.scrolloff = 8
vim.o.mousescroll = "ver:1,hor:0"
vim.o.updatetime = 50 -- multiple-cursors syncs on CursorHold
vim.o.timeoutlen = 300
vim.o.redrawtime = 10000 -- treesitter on big files
vim.o.cmdheight = 0
vim.o.laststatus = 3
vim.o.termguicolors = true

vim.o.number = true
vim.o.relativenumber = true
vim.o.numberwidth = 1
vim.o.signcolumn = "yes"
vim.o.expandtab = true
vim.o.shiftwidth = 4
vim.o.tabstop = 4
vim.o.smartindent = true
vim.o.linebreak = true
vim.o.breakindent = true
vim.o.showbreak = "↪ "
vim.o.wrapscan = false
vim.o.spelllang = "en_us"

-- nvim-ufo needs folds open by default.
vim.o.foldcolumn = "1"
vim.o.foldlevel = 99
vim.o.foldlevelstart = 99

-- Statusline path shows the symlink chain (dotfiles link into a store),
-- ending at <repo>/<path> or a ~-relative path.
local function hop(path)
  local rest = path:match("^/nix/store/[a-z0-9]+%-(.+)$")
  return rest and ("store:" .. rest) or vim.fn.fnamemodify(path, ":~")
end

function _G.statusline_path()
  local buf = vim.api.nvim_get_current_buf()
  local name = vim.api.nvim_buf_get_name(buf)
  if name == "" then return "[No Name]" end
  local cache = vim.b[buf].statusline_path
  if cache and cache.name == name then return cache.text end

  local hops, cur = {}, vim.fs.normalize(vim.fn.fnamemodify(name, ":p"))
  for _ = 1, 8 do -- capped in case of a link cycle
    local target = vim.uv.fs_readlink(cur)
    if not target then break end
    if not target:match("^/") then target = vim.fs.dirname(cur) .. "/" .. target end
    hops[#hops + 1] = hop(cur)
    cur = vim.fs.normalize(vim.fn.simplify(target))
  end
  local real = vim.fs.normalize(vim.uv.fs_realpath(cur) or cur)
  local git = vim.fs.find(".git", { upward = true, path = vim.fs.dirname(real) })[1]
  local root = git and vim.fs.dirname(git)
  hops[#hops + 1] = root and (vim.fs.basename(root) .. real:sub(#root + 1)) or vim.fn.fnamemodify(real, ":~")

  local text = table.concat(hops, " → ")
  vim.b[buf].statusline_path = { name = name, text = text }
  return text
end
vim.o.statusline = "%{v:lua.statusline_path()}%m%=%l:%c"
