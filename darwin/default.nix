{
  inputs,
  pkgs,
  self,
  ...
}:
let
  user = "andrewgazelka";
in
{
  imports = [
    ./blocklist.nix
    ./defaults.nix
    ./homebrew.nix
  ];

  nixpkgs.hostPlatform = "aarch64-darwin";
  nixpkgs.config.allowUnfree = true;

  networking = {
    hostName = "hydra";
    computerName = "hydra";
    localHostName = "hydra";
  };

  nix = {
    package = pkgs.nix;
    registry.nixpkgs.flake = inputs.nixpkgs;
    nixPath = [ "nixpkgs=flake:nixpkgs" ];
    gc.automatic = true;
    settings = {
      experimental-features = [
        "nix-command"
        "flakes"
      ];
      trusted-users = [ "@admin" ];
      # Off by default on darwin. Without it, an interrupted content-addressed
      # build leaves unowned half-written outputs in /nix/store.
      sandbox = true;
      warn-dirty = false;
      keep-outputs = true;
      keep-derivations = true;
      connect-timeout = 5;
      http-connections = 128;
      max-jobs = 32;
      max-substitution-jobs = 64;
      download-buffer-size = 268435456;
      # No store hardlinking: dyld resolves @rpath through a hardlink's other
      # path, which the build sandbox then blocks.
      auto-optimise-store = false;
    };
    # Written from `gh auth token` on every switch (home/default.nix).
    extraOptions = ''
      !include /Users/${user}/.config/nix/access-tokens.conf
    '';
  };

  users.knownUsers = [ user ];
  users.users.${user} = {
    uid = 501;
    gid = 20;
    home = "/Users/${user}";
    shell = pkgs.nushell;
  };
  system.primaryUser = user;

  home-manager = {
    useGlobalPkgs = true;
    extraSpecialArgs = { inherit inputs self; };
    users.${user} = import ../home;
  };

  environment.shells = [ pkgs.nushell ];
  environment.systemPackages = [ pkgs.nushell ];

  fonts.packages = with pkgs; [
    fira-code
    inter
    iosevka-bin
    nerd-fonts.fira-code
    nerd-fonts.iosevka-term-slab
  ];

  security.pam.services.sudo_local = {
    touchIdAuth = true;
    reattach = true; # Touch ID inside tmux
  };
  security.sudo.extraConfig = ''
    ${user} ALL=(ALL) NOPASSWD: ALL
  '';

  # Mount the /nix volume with retries; the installer's one-shot plist loses
  # the race against DiskArbitration on the first boot after a macOS update.
  launchd.daemons.darwin-store.serviceConfig =
    let
      volume = "3B0C6572-6749-4C11-9EA6-F92E5E669835";
    in
    {
      Label = "org.nixos.darwin-store";
      ProgramArguments = [
        "/bin/sh"
        "-c"
        ''
          /bin/test -d /nix/store && exit 0
          /usr/bin/security find-generic-password -s '${volume}' -w \
            | /usr/sbin/diskutil apfs unlockVolume '${volume}' -mountpoint /nix -stdinpassphrase
        ''
      ];
      RunAtLoad = true;
      KeepAlive.SuccessfulExit = false;
      ThrottleInterval = 5;
    };

  launchd.daemons.limit-maxfiles.serviceConfig = {
    Label = "limit.maxfiles";
    ProgramArguments = [
      "/bin/launchctl"
      "limit"
      "maxfiles"
      "65536"
      "524288"
    ];
    RunAtLoad = true;
  };

  # Keep Spotlight and fseventsd off the store; builds churn it constantly.
  system.activationScripts.postActivation.text = ''
    /usr/bin/mdutil -i on / >/dev/null 2>&1 || true
    if [ -d /nix ]; then
      mkdir -p /nix/.fseventsd
      touch /nix/.fseventsd/no_log /nix/.metadata_never_index
      /usr/bin/mdutil -i off /nix >/dev/null 2>&1 || true
    fi
  '';

  documentation.doc.enable = false;
  system.tools.darwin-uninstaller.enable = false;

  system.configurationRevision = self.rev or self.dirtyRev or null;
  system.stateVersion = 6;
}
