local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
if not vim.uv.fs_stat(lazypath) then
  vim.fn.system({ "git", "clone", "--filter=blob:none", "--branch=stable",
    "https://github.com/folke/lazy.nvim.git", lazypath })
end
vim.opt.rtp:prepend(lazypath)

require("config.options")
require("config.autocmds")
require("lazy").setup("plugins", {
  ui = { border = "rounded" },
  change_detection = { notify = false },
})
require("config.keymaps")
