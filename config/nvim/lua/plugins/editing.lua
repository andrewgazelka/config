return {
  { "tpope/vim-surround" },

  {
    "dkarter/bullets.vim",
    ft = { "markdown", "text", "gitcommit" },
    init = function() vim.g.bullets_outline_levels = { "num", "abc", "std-" } end,
  },

  {
    "windwp/nvim-autopairs",
    event = "InsertEnter",
    config = function()
      require("nvim-autopairs").setup({ fast_wrap = {} })
      -- Tab jumps over a closer (or markdown `*`/`**`), IntelliJ-style.
      vim.keymap.set("i", "<Tab>", function()
        local col = vim.api.nvim_win_get_cursor(0)[2]
        local line = vim.api.nvim_get_current_line()
        if line:sub(col + 1, col + 2) == "**" then return "<Right><Right>" end
        if line:sub(col + 1, col + 1):match("[%)%]}\"'`>*]") then return "<Right>" end
        return "<Tab>"
      end, { expr = true })
    end,
  },

  {
    "saghen/blink.cmp",
    dependencies = { "rafamadriz/friendly-snippets" },
    version = "v1.*",
    opts = {
      enabled = function() return not vim.tbl_contains({ "markdown", "typst" }, vim.bo.filetype) end,
      keymap = {
        preset = "default",
        ["<CR>"] = { "accept", "fallback" },
        ["<Tab>"] = { "accept", "snippet_forward", "fallback" },
        ["<C-u>"] = { "scroll_documentation_up", "fallback" },
        ["<C-d>"] = { "scroll_documentation_down", "fallback" },
      },
    },
  },

  {
    "brenton-leighton/multiple-cursors.nvim",
    version = "*",
    opts = {},
    keys = {
      { "<M-n>", "<Cmd>MultipleCursorsAddJumpNextMatch<CR>", mode = { "n", "x" }, desc = "Add cursor at next match" },
      { "<M-N>", "<Cmd>MultipleCursorsAddMatches<CR>", mode = { "n", "x" }, desc = "Add cursors at all matches" },
      { "<M-x>", "<Cmd>MultipleCursorsJumpNextMatch<CR>", mode = { "n", "x" }, desc = "Skip to next match" },
      { "<up>", "<Cmd>MultipleCursorsAddUp<CR>", mode = { "n", "x" }, desc = "Add cursor up" },
      { "<down>", "<Cmd>MultipleCursorsAddDown<CR>", mode = { "n", "x" }, desc = "Add cursor down" },
      { "<C-Up>", "<Cmd>MultipleCursorsAddUp<CR>", mode = "i", desc = "Add cursor up" },
      { "<C-Down>", "<Cmd>MultipleCursorsAddDown<CR>", mode = "i", desc = "Add cursor down" },
      { "<C-LeftMouse>", "<Cmd>MultipleCursorsMouseAddDelete<CR>", mode = { "n", "i" }, desc = "Add/remove cursor" },
      { "<leader>ml", "<Cmd>MultipleCursorsLock<CR>", mode = { "n", "x" }, desc = "Lock cursors" },
    },
  },
}
