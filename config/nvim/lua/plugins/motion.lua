return {
  "folke/flash.nvim",
  event = "VeryLazy",
  opts = {
    highlight = { backdrop = false },
    modes = {
      -- f/F/t/T always show jump labels, on any letter.
      char = { jump_labels = true, label = { exclude = "" }, highlight = { backdrop = false } },
      treesitter_search = { label = { before = false } },
    },
  },
  keys = {
    { "<leader><leader>s", function() require("flash").jump() end, mode = { "n", "x", "o" }, desc = "Flash" },
    {
      "<leader><leader>w",
      function() require("flash").jump({ search = { mode = "search", max_length = 0 }, label = { after = { 0, 0 } }, pattern = "^" }) end,
      mode = { "n", "x", "o" },
      desc = "Flash to line start",
    },
  },
}
