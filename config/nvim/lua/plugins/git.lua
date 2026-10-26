return {
  "lewis6991/gitsigns.nvim",
  opts = {
    signs = {
      add = { text = "▎" },
      change = { text = "▎" },
      delete = { text = "▎" },
      topdelete = { text = "▎" },
      changedelete = { text = "▎" },
      untracked = { text = "▎" },
    },
    current_line_blame_formatter = "<author> • <author_time:%R> • <summary>",
    watch_gitdir = { follow_files = false },
    update_debounce = 200,
    max_file_length = 10000,
  },
  keys = {
    { "<leader>gt", "<cmd>Gitsigns toggle_current_line_blame<cr>", desc = "Toggle inline blame" },
    { "<leader>gB", "<cmd>Gitsigns blame<cr>", desc = "Blame sidebar" },
    { "<leader>gb", "<cmd>Gitsigns toggle_linehl<cr>", desc = "Toggle line highlight" },
    { "<leader>gd", "<cmd>Gitsigns diffthis<cr>", desc = "Diff against index" },
  },
  event = { "BufReadPre", "BufNewFile" },
}
