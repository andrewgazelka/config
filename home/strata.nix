# strata writes the dotfiles in config/. This agent applies the checkout at
# login and again on every save, so ~ always matches the repo.
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

  # The store paths config/ links to, from this system's nixpkgs. Being in
  # the agent's closure keeps them alive for as long as the system is.
  facts = pkgs.writeText "strata-facts.json" (
    builtins.toJSON (import ../config/nix/facts.nix { inherit system pkgs; })
  );
in
{
  home.packages = [ strata ];

  launchd.agents.strata = {
    enable = true;
    config = {
      ProgramArguments = [
        "${strata}/bin/strata"
        "--config"
        "${repo}/config"
        "--config-optional"
        "${repo}/private/config"
        "--facts"
        "pkgs=${facts}"
        "watch"
      ];
      KeepAlive = true;
      RunAtLoad = true;
      StandardOutPath = "${config.home.homeDirectory}/Library/Logs/strata.log";
      StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/strata.log";
    };
  };
}
