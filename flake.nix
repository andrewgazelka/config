{
  description = "Andrew's Mac: nix-darwin, home-manager, and dotfiles in strata";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nix-darwin = {
      url = "github:nix-darwin/nix-darwin";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    lifelog = {
      url = "github:andrewgazelka/lifelog";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    notify = {
      url = "github:andrewgazelka/notify";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    # A pinned toolchain with the aarch64-apple-darwin std, so Linux can
    # build the Mac's strata.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      nix-darwin,
      home-manager,
      crane,
      rust-overlay,
      ...
    }@inputs:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      src =
        craneLib:
        nixpkgs.lib.cleanSourceWith {
          src = ./strata;
          filter =
            path: type: craneLib.filterCargoSources path type || builtins.match ".*\\.(cfg|json)$" path != null;
        };

      build =
        pkgs:
        let
          craneLib = crane.mkLib pkgs;
          common = {
            src = src craneLib;
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly common;
        in
        {
          strata = craneLib.buildPackage (
            common
            // {
              inherit cargoArtifacts;
              doCheck = false;
            }
          );
          clippy = craneLib.cargoClippy (
            common
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            }
          );
          # Unit tests, the golden outputs and the crash convergence test.
          tests = craneLib.cargoTest (common // { inherit cargoArtifacts; });
          fmt = craneLib.cargoFmt { src = src craneLib; };
        };

      # The Mac binary, built on Linux: nixpkgs' rustc release from
      # rust-overlay, with the darwin std, linked by cargo-zigbuild (zig
      # ships the Mach-O linker, libSystem stubs and an ad-hoc signature).
      # strata links no framework, so no SDK is needed.
      darwinCross =
        pkgs:
        let
          toolchain =
            (pkgs.appendOverlays [ rust-overlay.overlays.default ])
            .rust-bin.stable.${pkgs.rustc.version}.minimal.override
              { targets = [ "aarch64-apple-darwin" ]; };
          craneLib = (crane.mkLib pkgs).overrideToolchain (_: toolchain);
        in
        craneLib.buildPackage {
          pname = "strata-aarch64-darwin";
          src = src craneLib;
          strictDeps = true;
          cargoArtifacts = null;
          CARGO_BUILD_TARGET = "aarch64-apple-darwin";
          nativeBuildInputs = [
            pkgs.cargo-zigbuild
            pkgs.zig
          ];
          preBuild = ''
            export XDG_CACHE_HOME=$TMPDIR/xdg-cache ZIG_GLOBAL_CACHE_DIR=$TMPDIR/zig-cache ZIG_LOCAL_CACHE_DIR=$TMPDIR/zig-local
          '';
          cargoBuildCommand = "cargo zigbuild --profile release";
          doCheck = false;
          # Rewriting store paths in the binary, or stripping it with Linux
          # tools, would break its code signature.
          dontFixup = true;
          doNotRemoveReferencesToRustToolchain = true;
          doNotRemoveReferencesToVendorDir = true;
        };
    in
    {
      # The whole Mac as one module, so another flake can build on it:
      # darwinSystem { modules = [ config.darwinModules.default ./more.nix ]; }.
      # Home modules join through home-manager.users.<name>.imports.
      darwinModules.default = {
        imports = [
          home-manager.darwinModules.home-manager
          ./darwin
        ];
        _module.args = { inherit inputs self; };
      };
      # The home-manager half on its own; it reads `inputs` and `self` from
      # extraSpecialArgs, which darwinModules.default sets.
      homeModules.default = ./home;

      darwinConfigurations.hydra = nix-darwin.lib.darwinSystem {
        modules = [ self.darwinModules.default ];
      };

      packages = forAll (
        pkgs:
        let
          inherit (build pkgs) strata;
        in
        {
          inherit strata;
          default = strata;
        }
        // nixpkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          strata-aarch64-darwin = darwinCross pkgs;
        }
      );

      checks = forAll (pkgs: {
        inherit (build pkgs)
          strata
          clippy
          tests
          fmt
          ;
        no-secrets = pkgs.runCommand "no-secrets" { nativeBuildInputs = [ pkgs.gitleaks ]; } ''
          gitleaks detect --no-banner --no-git --source ${self}
          touch $out
        '';
      });

      formatter = forAll (pkgs: pkgs.nixfmt-tree);
    };
}
