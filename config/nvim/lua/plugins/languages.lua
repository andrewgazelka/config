return {
  {
    "nvim-treesitter/nvim-treesitter",
    lazy = false,
    build = ":TSUpdate",
    config = function()
      -- Fence languages are preinstalled so markdown injections colour on first render.
      require("nvim-treesitter").install({
        "git_config", "gitcommit", "git_rebase", "gitignore", "gitattributes", "diff",
        "markdown", "markdown_inline", "bash", "nu",
        "rust", "lua", "python",
        "go", "c", "cpp", "json", "yaml", "toml", "html", "css", "sql",
      })
      vim.api.nvim_create_autocmd("FileType", {
        callback = function(args) pcall(vim.treesitter.start, args.buf) end,
      })
    end,
  },

  -- Notion-style code blocks and inline-code badges; everything else stays plain.
  {
    "MeanderingProgrammer/render-markdown.nvim",
    ft = "markdown",
    opts = {
      anti_conceal = { enabled = false },
      render_modes = true,
      win_options = {
        conceallevel = { default = 0, rendered = 0 },
        concealcursor = { default = "", rendered = "" },
      },
      code = {
        sign = false,
        conceal_delimiters = false,
        language = false,
        style = "normal",
        width = "block",
        min_width = 50,
        left_pad = 2,
        right_pad = 2,
        inline_pad = 1,
      },
      heading = { enabled = false },
      bullet = { enabled = false },
      checkbox = { enabled = false },
      dash = { enabled = false },
      link = { enabled = false },
      quote = { enabled = false },
      pipe_table = { enabled = false },
    },
  },

  {
    "kaarmu/typst.vim",
    ft = "typst",
    init = function()
      vim.g.typst_conceal = 1 -- also enables math/emoji conceal
      vim.g.typst_folding = 1
      vim.g.typst_embedded_languages = { "python", "lua", "bash", "c", "cpp", "go", "java" }
    end,
  },
}
