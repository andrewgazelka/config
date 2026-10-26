{
  config,
  lib,
  pkgs,
  ...
}:
{
  imports = [
    ./darwin.nix
    ./packages.nix
    ./shell.nix
    ./strata.nix
  ];

  home.stateVersion = "23.11";

  home.sessionPath = [
    "$HOME/.local/bin"
    "$HOME/.cargo/bin"
    "$HOME/.bun/bin"
    "/opt/homebrew/bin"
  ];

  # zsh's environment; nushell's is `session` in config/shell.cfg.
  home.sessionVariables = {
    EDITOR = "nvim";
    VISUAL = "nvim";
    PAGER = "moor";
    MOOR = "--wrap --quit-if-one-screen";
    LESS = "-R";
    LC_COLLATE = "C"; # byte-order sorting everywhere
    RUST_BACKTRACE = "1";
  };

  # Give nix a GitHub token so flake updates aren't rate limited.
  home.activation.nixGithubToken = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
    if token=$(${lib.getExe pkgs.gh} auth token 2>/dev/null); then
      mkdir -p ${config.xdg.configHome}/nix
      (umask 077; echo "access-tokens = github.com=$token" > ${config.xdg.configHome}/nix/access-tokens.conf)
    fi
  '';

  manual.manpages.enable = false;
  news.display = "silent";
}
