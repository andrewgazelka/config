# Store paths the config links to, for one system's nixpkgs. home/strata.nix
# writes the table to ~/.config/strata/facts/pkgs.json (config/strata.cfg
# reads it there), which keeps the paths alive with the system.
{ pkgs, ... }:
let
  # Prompt, jump and history hooks for nushell, generated from the binaries.
  vendor =
    pkgs.runCommand "nushell-vendor"
      {
        nativeBuildInputs = [
          pkgs.starship
          pkgs.zoxide
          pkgs.atuin
        ];
      }
      ''
        export HOME=$TMPDIR
        mkdir -p $out/autoload
        starship init nu > $out/autoload/starship.nu
        zoxide init nushell > $out/autoload/zoxide.nu
        atuin init nu --disable-up-arrow > $out/autoload/atuin.nu
      '';
in
{
  delta = "${pkgs.delta}";
  gh = "${pkgs.gh}";
  git-lfs = "${pkgs.git-lfs}";
  nix-direnv = "${pkgs.nix-direnv}";
  nushell-vendor = "${vendor}";
  openssh = "${pkgs.openssh}";
}
