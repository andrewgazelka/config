# macOS: window manager, background agents, default apps.
{
  config,
  inputs,
  lib,
  pkgs,
  ...
}:
let
  logs = "${config.home.homeDirectory}/Library/Logs";
  lifelog = inputs.lifelog.packages.${pkgs.stdenv.hostPlatform.system}.lifelog;

  # Browser tabs on these sites get redirected; apps that talk to them
  # (Beeper, OAuth) keep working, unlike the DNS sinkhole in darwin/blocklist.nix.
  blockedTabs = [
    "threads.net"
    "tiktok.com"
    "facebook.com"
    "bsky.app"
    "pinterest.com"
    "tumblr.com"
  ];
  tabBlocklist = pkgs.writeShellScript "tab-blocklist" ''
    while :; do
      /usr/bin/osascript -l JavaScript ${./tab-blocklist.js} \
        ${lib.escapeShellArgs blockedTabs} || true
      sleep 1
    done
  '';
in
{
  # Its config is ~/.aerospace.toml, from config/aerospace.cfg.
  programs.aerospace = {
    enable = true;
    launchd.enable = true;
  };

  launchd.agents = {
    lifelog = {
      enable = true;
      config = {
        ProgramArguments = [
          (lib.getExe lifelog)
          "record"
          "--listen"
          "127.0.0.1:5599"
        ];
        KeepAlive = true;
        RunAtLoad = true;
        StandardErrorPath = "${logs}/lifelog.log";
      };
    };

    tab-blocklist = {
      enable = true;
      config = {
        ProgramArguments = [ "${tabBlocklist}" ];
        KeepAlive = true;
        RunAtLoad = true;
        StandardErrorPath = "${logs}/tab-blocklist.log";
      };
    };
  };

  # Ghostty is the default terminal.
  home.activation.defaultApps = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
    ${lib.getExe pkgs.duti} -s com.mitchellh.ghostty public.unix-executable shell || true
    ${lib.getExe pkgs.duti} -s com.mitchellh.ghostty .command all || true
  '';
}
