# yabai + skhd, replacing AeroSpace. The keys mirror the old AeroSpace ones.
#
# yabai comes from AhsanFazal/yabai: upstream (asmvik/yabai) has had no
# commits since June and its scripting addition can't find the Dock's
# functions on macOS 27, so space switching silently does nothing. The fork
# carries the macOS 27.0.1 and 27.2 Dock signatures. Its version string is
# still upstream's 7.1.25.
#
# The scripting addition needs SIP partly off
# (`csrutil enable --without fs --without debug --without nvram` from
# Recovery) and `nvram boot-args=-arm64e_preview_abi`. Without them yabai
# still tiles, but can't switch, create or move spaces.
{ lib, pkgs, ... }:
let
  yabai = pkgs.yabai.overrideAttrs (old: {
    version = "7.1.25-unstable-2026-10-31";
    src = pkgs.fetchFromGitHub {
      owner = "AhsanFazal";
      repo = "yabai";
      rev = "ad8d5adfdc39c76399599ae79a264f2525f6ca5e";
      hash = "sha256-berfA3vYih1YuCCSBEvpocCn2EPj+ZpftPmsE65ILWo=";
    };
    # `yabai --version` prints upstream's 7.1.25, not this version.
    doInstallCheck = false;
  });
  jq = lib.getExe pkgs.jq;
  y = "${yabai}/bin/yabai";

  spaces = lib.range 1 10;
  key = n: toString (lib.mod n 10); # alt-0 is space 10
  # Each key's stack neighbour, for the stacked layout (alt-a), where
  # west/south/north/east find nothing.
  directions = {
    h = {
      dir = "west";
      stack = "prev";
    };
    j = {
      dir = "south";
      stack = "next";
    };
    k = {
      dir = "north";
      stack = "prev";
    };
    l = {
      dir = "east";
      stack = "next";
    };
  };
  perDirection = f: lib.concatStrings (lib.mapAttrsToList f directions);
  perSpace = f: lib.concatMapStrings f spaces;

  barHeight = 32;
  bar = lib.getExe pkgs.sketchybar;
  spaceScript = pkgs.writeShellScript "sketchybar-space" ''
    if [ "$SELECTED" = true ]; then
      ${bar} --set "$NAME" background.drawing=on icon.color=0xff1e1e2e
    else
      ${bar} --set "$NAME" background.drawing=off icon.color=0xffcdd6f4
    fi
  '';
  frontAppScript = pkgs.writeShellScript "sketchybar-front-app" ''
    [ "$SENDER" = front_app_switched ] && ${bar} --set "$NAME" label="$INFO"
  '';
  clockScript = pkgs.writeShellScript "sketchybar-clock" ''
    ${bar} --set "$NAME" label="$(date '+%a %d %b  %H:%M')"
  '';
  batteryScript = pkgs.writeShellScript "sketchybar-battery" ''
    pct=$(/usr/bin/pmset -g batt | /usr/bin/grep -Eo '[0-9]+%' | head -1)
    case "$(/usr/bin/pmset -g batt)" in *"AC Power"*) pct="⚡ $pct" ;; esac
    ${bar} --set "$NAME" label="$pct"
  '';
in
{
  services.yabai = {
    enable = true;
    package = yabai;
    enableScriptingAddition = true;
    config = {
      layout = "bsp";
      window_placement = "second_child";
      mouse_follows_focus = "off";
      focus_follows_mouse = "off";
      window_animation_duration = 0;
      window_gap = 0;
      external_bar = "all:${toString barHeight}:0";
    };
    extraConfig = ''
      # The Dock loses the scripting addition whenever it restarts.
      ${y} -m signal --add event=dock_did_restart action="sudo ${y} --load-sa"
      sudo ${y} --load-sa

      # Ten spaces, like AeroSpace's ten workspaces.
      for _ in $(seq "$(${y} -m query --spaces | ${jq} length)" 9); do
        ${y} -m space --create
      done

      ${y} -m rule --add app='^(FaceTime|OBS Studio|OBS|Super Easy Timer|System Settings)$' manage=off
      ${y} -m rule --add app='^(Linear|Microsoft Excel|Notion)$' manage=on
      ${y} -m rule --apply
    '';
  };

  # The top bar: spaces (current one highlighted), the front app, battery and
  # clock. The native menu bar auto-hides (_HIHideMenuBar), so this sits in
  # its place; yabai's external_bar keeps windows below it.
  services.sketchybar = {
    enable = true;
    config = ''
      #!/bin/sh
      bar=${bar}
      $bar --bar height=${toString barHeight} position=top color=0xd01e1e2e \
        padding_left=8 padding_right=8 topmost=window
      $bar --default updates=when_shown \
        icon.font="Inter:Semibold:13.0" label.font="Inter:Semibold:13.0" \
        icon.color=0xffcdd6f4 label.color=0xffcdd6f4 \
        padding_left=4 padding_right=4 label.padding_left=4 label.padding_right=4

      for n in ${lib.concatMapStringsSep " " toString spaces}; do
        $bar --add space space.$n left \
          --set space.$n associated_space=$n icon=$n \
            icon.padding_left=8 icon.padding_right=8 \
            background.color=0xff89b4fa background.corner_radius=5 background.height=22 \
            background.drawing=off script=${spaceScript}
      done

      $bar --add item front_app left \
        --set front_app icon.drawing=off label.padding_left=12 script=${frontAppScript} \
        --subscribe front_app front_app_switched

      $bar --add item clock right \
        --set clock update_freq=10 icon.drawing=off script=${clockScript} \
        --add item battery right \
        --set battery update_freq=120 icon.drawing=off script=${batteryScript} \
        --subscribe battery system_woke power_source_change

      $bar --update
    '';
  };
  launchd.user.agents.sketchybar.serviceConfig.EnvironmentVariables.SHELL = "/bin/sh";

  # skhd and yabai signals run commands with $SHELL -c. The login shell is
  # nushell, which has no `||` or `$(...)`, so the bindings below failed
  # silently under it.
  launchd.user.agents.skhd.serviceConfig.EnvironmentVariables.SHELL = "/bin/sh";
  launchd.user.agents.yabai.serviceConfig.EnvironmentVariables.SHELL = "/bin/sh";

  services.skhd = {
    enable = true;
    skhdConfig =
      perSpace (n: ''
        alt - ${key n} : ${y} -m space --focus ${toString n}
        alt + shift - ${key n} : ${y} -m window --space ${toString n}
      '')
      + perDirection (
        k: d: ''
          alt - ${k} : ${y} -m window --focus ${d.dir} || ${y} -m window --focus stack.${d.stack}
          ctrl + alt + shift - ${k} : ${y} -m window --swap ${d.dir}
          alt + shift - ${k} : ${y} -m window --warp ${d.dir}
        ''
      )
      + ''
        alt - tab : ${y} -m space --focus recent
        alt - 0x32 : ${y} -m window --focus recent
        alt - a : ${y} -m space --layout "$(${y} -m query --spaces --space | ${jq} -r 'if .type == "bsp" then "stack" else "bsp" end')"
        alt + shift - e : ${y} -m space --balance
        # Shrink or grow the focused window by 50px, whichever side its split is on.
        alt + shift - 0x1B : ${y} -m window --resize right:-50:0 || ${y} -m window --resize left:50:0
        alt + shift - 0x18 : ${y} -m window --resize right:50:0 || ${y} -m window --resize left:-50:0
      '';
  };
}
