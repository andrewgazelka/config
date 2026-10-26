local map = vim.keymap.set

map("n", "<C-c>", "<cmd>q!<cr>", { desc = "Quit without saving" })
map("v", "q", "y:q<CR>", { desc = "Yank selection and quit" })

-- macOS-style Cmd/Option keys
map({ "n", "v" }, "<D-a>", "ggVG", { desc = "Select all" })
map("i", "<D-a>", "<Esc>ggVG", { desc = "Select all" })
map({ "n", "v" }, "<D-e>", "<cmd>q<cr>", { desc = "Quit" })
map("i", "<D-e>", "<Esc><cmd>q<cr>", { desc = "Quit" })
map({ "i", "c" }, "<M-BS>", "<C-w>", { desc = "Delete previous word" })

map("n", "<leader>st", "<cmd>setlocal spell!<cr>", { desc = "Toggle spell check" })
