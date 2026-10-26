-- Local plugin, see lua/agent/init.lua.
return {
  dir = vim.fn.stdpath("config"),
  name = "claude-agent",
  event = "VeryLazy",
  opts = {},
  config = function(_, opts) require("agent").setup(opts) end,
}
