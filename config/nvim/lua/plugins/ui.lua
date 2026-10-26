return {
  {
    "nickkadutskyi/jb.nvim",
    lazy = false,
    priority = 1000,
    config = function()
      require("jb").setup({ transparent = true })
      vim.cmd.colorscheme("jb")
    end,
  },

  {
    "folke/snacks.nvim",
    lazy = false,
    priority = 1000,
    opts = {
      picker = { enabled = true, sources = { files = { hidden = true }, grep = { hidden = true } } },
      bigfile = { enabled = true },
      quickfile = { enabled = true },
      statuscolumn = { enabled = true },
      dashboard = {
        enabled = true,
        sections = {
          { section = "keys", gap = 0, padding = 0 },
          { section = "recent_files", limit = 8, gap = 0, padding = 0 },
          { section = "projects", gap = 0, padding = 0 },
          { section = "startup" },
        },
      },
    },
    keys = {
      { "<leader>ff", function() Snacks.picker.files() end, desc = "Find files" },
      { "<leader>fg", function() Snacks.picker.grep() end, desc = "Grep" },
      { "<leader>fw", function() Snacks.picker.grep_word() end, desc = "Grep word" },
      { "<leader>fb", function() Snacks.picker.buffers() end, desc = "Buffers" },
      { "<leader>fr", function() Snacks.picker.recent() end, desc = "Recent files" },
    },
  },

  {
    "nvim-tree/nvim-tree.lua",
    dependencies = { "nvim-tree/nvim-web-devicons" },
    lazy = false, -- hijacks `nvim <dir>`
    keys = { { "<leader>e", "<cmd>NvimTreeToggle<cr>", desc = "File explorer" } },
    init = function()
      vim.g.loaded_netrw = 1
      vim.g.loaded_netrwPlugin = 1
    end,
    opts = {
      hijack_cursor = true,
      view = { width = 35 },
      update_focused_file = { enable = true },
      git = { ignore = false },
      filters = { custom = { "^.git$" } },
      renderer = { indent_markers = { enable = true } },
    },
  },

  {
    "kevinhwang91/nvim-ufo",
    dependencies = { "kevinhwang91/promise-async" },
    event = "VeryLazy",
    config = function()
      local ufo = require("ufo")
      ufo.setup({
        provider_selector = function() return { "treesitter", "indent" } end,
        open_fold_hl_timeout = 0,
      })
      vim.keymap.set("n", "zR", ufo.openAllFolds, { desc = "Open all folds" })
      vim.keymap.set("n", "zM", ufo.closeAllFolds, { desc = "Close all folds" })
      vim.keymap.set("n", "zr", ufo.openFoldsExceptKinds, { desc = "Fold less" })
      vim.keymap.set("n", "zm", ufo.closeFoldsWith, { desc = "Fold more" })
      vim.keymap.set("n", "zp", ufo.peekFoldedLinesUnderCursor, { desc = "Peek fold" })
    end,
  },

  {
    "nvim-treesitter/nvim-treesitter-context",
    opts = { max_lines = 3, min_window_height = 20 },
  },
}
