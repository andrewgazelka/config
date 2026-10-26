# Store paths the config links to, for one system. `strata nix-facts` passes
# only the system, and nixpkgs comes from the repo's flake.lock;
# home/strata.nix also passes the system's own pkgs.
{
  system,
  pkgs ?
    let
      lock = (builtins.fromJSON (builtins.readFile ../../flake.lock)).nodes.nixpkgs.locked;
      nixpkgs = builtins.fetchTarball {
        url = "https://github.com/${lock.owner}/${lock.repo}/archive/${lock.rev}.tar.gz";
        sha256 = lock.narHash;
      };
    in
    import nixpkgs {
      inherit system;
      config.allowUnfree = true;
    },
}:
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
