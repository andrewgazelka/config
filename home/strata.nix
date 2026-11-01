# strata writes the dotfiles in config/. This agent applies the checkout at
# login and again on every save, so ~ always matches the repo. It names only
# the config, as a hand run from the repo root does: the layers and fact
# tables are declared in config/strata.cfg, not passed here.
{
  config,
  pkgs,
  self,
  ...
}:
let
  repo = "/Volumes/Projects/nix";
  inherit (pkgs.stdenv.hostPlatform) system;
  strata = self.packages.${system}.strata;
in
{
  home.packages = [ strata ];

  # The store paths config/ links to, from this system's nixpkgs. Linked
  # from the home generation, which keeps them alive as long as it is.
  xdg.configFile."strata/facts/pkgs.json".text = builtins.toJSON (
    import ../config/nix/facts.nix { inherit pkgs; }
  );

  launchd.agents.strata = {
    enable = true;
    config = {
      ProgramArguments = [
        "${strata}/bin/strata"
        "--config"
        "${repo}/config"
        "watch"
      ];
      KeepAlive = true;
      RunAtLoad = true;
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/strata.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/strata.log";
    };
  };
}
