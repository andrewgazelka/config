# nushell is the login shell; zsh stays for scripts and agents. Their config
# files come from config/ (strata); this installs the tools and zsh's hooks.
{
  config,
  lib,
  pkgs,
  ...
}:
{
  home.packages = [
    pkgs.nushell
    pkgs.tmux
    pkgs.vivid # LS_COLORS for nushell's theme
  ];

  programs.zsh = {
    enable = true;
    # Agents (CLAUDECODE=1) get plain ls.
    initContent = ''
      if [[ -z ''${CLAUDECODE-} ]]; then
        alias ls='eza --git --time-style=relative' ll='ls -l' la='ls -a' lt='ls --tree'
      fi
    '';
  };

  programs.starship.enable = true;

  programs.atuin = {
    enable = true;
    flags = [ "--disable-up-arrow" ];
  };
  # atuin's own daemon.enable would write its config file, which is strata's.
  launchd.agents.atuin-daemon = {
    enable = true;
    config = {
      ProgramArguments = [
        (lib.getExe config.programs.atuin.package)
        "daemon"
        "start"
      ];
      KeepAlive = {
        Crashed = true;
        SuccessfulExit = false;
      };
      ProcessType = "Background";
    };
  };

  programs.zoxide.enable = true;
  programs.direnv = {
    enable = true;
    nix-direnv.enable = false; # strata links it
  };

  programs.eza = {
    enable = true;
    git = true;
    icons = "auto";
    enableZshIntegration = false;
  };

  programs.fzf = {
    enable = true;
    historyWidget.command = ""; # atuin owns ctrl-r
  };
  programs.bat.enable = true;
  programs.yazi = {
    enable = true;
    shellWrapperName = "y";
  };
}
