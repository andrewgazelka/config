# yabai + skhd, replacing AeroSpace. The keys mirror the old AeroSpace ones.
#
# yabai comes from andrewgazelka/yabai, a fork of AhsanFazal/yabai: upstream
# (asmvik/yabai) has had no commits since June and its scripting addition
# can't find the Dock's functions on macOS 27, so space switching silently
# does nothing. AhsanFazal's fork carries the macOS 27.0.1 and 27.2 Dock
# signatures; ours adds our own fixes on top (freezing screen updates while
# setting frames, so the wallpaper doesn't flash between windows). Its version
# string is still upstream's 7.1.25.
#
# The scripting addition needs SIP partly off
# (`csrutil enable --without fs --without debug --without nvram` from
# Recovery) and `nvram boot-args=-arm64e_preview_abi`. Without them yabai
# still tiles, but can't switch, create or move spaces.
{ inputs, lib, pkgs, ... }:
let
  on = false;
  yabai = pkgs.yabai.overrideAttrs (old: {
    version = "7.1.25-unstable-${lib.substring 0 8 inputs.yabai.lastModifiedDate}";
    src = inputs.yabai; # pinned in flake.lock
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

  bar = lib.getExe pkgs.sketchybar;
  # Follows light/dark mode: a solid wallpaper in Ghostty's custom-light /
  # custom-dark background (config/ghostty.cfg).
  themeScript = pkgs.writeShellScript "wallpaper-theme" ''
    if /usr/bin/defaults read -g AppleInterfaceStyle 2>/dev/null | /usr/bin/grep -q Dark; then
      ${desktoppr} ${wallpaper "1e1e1e"}
    else
      ${desktoppr} ${wallpaper "f9f9f9"}
    fi
  '';
  # A solid wallpaper in Ghostty's background colour, so the gap that shows
  # for a moment while an app redraws at a new size matches the terminal.
  wallpaper =
    color:
    pkgs.runCommand "wallpaper-${color}.png" { nativeBuildInputs = [ pkgs.imagemagick ]; } ''
      magick -size 64x64 xc:'#${color}' $out
    '';
  desktoppr = lib.getExe pkgs.desktoppr;
  # Grow or shrink the focused window: `resize <px> w|h`. The edge is picked
  # from the frame: the right/bottom edge unless it touches the display edge,
  # then the left/top one. Don't try one edge and fall back on failure: with no
  # fence on that side yabai sometimes moves the opposite fence the wrong way
  # and still exits 0, so repeated presses flip back and forth.
  resizeScript = pkgs.writeShellScript "yabai-resize" ''
    d=$1
    set -- $( { ${y} -m query --windows --window; ${y} -m query --displays --window; } | ${jq} -rs --arg axis "$2" '
      .[0].frame as $w | .[1].frame as $f |
      if $axis == "h" then
        if $w.y + $w.h >= $f.y + $f.h - 1 then "top 0 -1" else "bottom 0 1" end
      else
        if $w.x + $w.w >= $f.x + $f.w - 1 then "left -1 0" else "right 1 0" end
      end')
    ${y} -m window --resize "$1:$(($2 * d)):$(($3 * d))"
  '';
in
{
  # Off for now (2026-11-03); flip back to true to re-enable yabai + skhd.
  services.yabai = {
    enable = on;
    package = yabai;
    enableScriptingAddition = true;
    config = {
      layout = "bsp";
      window_placement = "second_child";
      mouse_follows_focus = "off";
      focus_follows_mouse = "off";
      # Animation doesn't hide the gap while apps (Ghostty) redraw after a
      # resize; the wallpaper is Ghostty's background colour instead (below).
      window_animation_duration = 0;
      window_shadow = "off";
      window_gap = 0;
    };
    extraConfig = ''
      # The Dock loses the scripting addition whenever it restarts.
      ${y} -m signal --add event=dock_did_restart action="sudo ${y} --load-sa"
      sudo ${y} --load-sa

      # Ten spaces, like AeroSpace's ten workspaces. This runs on every yabai
      # start and launchd restarts yabai whenever it exits, so a create that
      # yabai's count doesn't see would pile up spaces (2000 once). Create one
      # at a time, re-count, and stop as soon as a create doesn't add one.
      n=$(${y} -m query --spaces | ${jq} length) || n=10
      while [ "$n" -lt 10 ]; do
        ${y} -m space --create || break
        m=$(${y} -m query --spaces | ${jq} length) || break
        if [ "$m" -le "$n" ]; then
          echo "yabairc: space --create left $n spaces, stopping" >&2
          break
        fi
        n=$m
      done

      ${y} -m rule --add app='^(FaceTime|OBS Studio|OBS|Super Easy Timer|System Settings)$' manage=off
      ${y} -m rule --add app='^(Linear|Microsoft Excel|Notion)$' manage=on
      ${y} -m rule --apply
    '';
  };

  # The native menu bar is used. sketchybar draws nothing; it only runs the
  # wallpaper script when light/dark mode changes.
  services.sketchybar = {
    enable = true;
    config = ''
      #!/bin/sh
      bar=${bar}
      $bar --bar hidden=on
      $bar --add event theme_changed AppleInterfaceThemeChangedNotification \
        --add item theme left --set theme drawing=off updates=on script=${themeScript} \
        --subscribe theme theme_changed
      ${themeScript}
    '';
  };
  launchd.user.agents.sketchybar.serviceConfig.EnvironmentVariables.SHELL = "/bin/sh";

  # skhd and yabai signals run commands with $SHELL -c. The login shell is
  # nushell, which has no `||` or `$(...)`, so the bindings below failed
  # silently under it.
  launchd.user.agents.skhd = lib.mkIf on { serviceConfig.EnvironmentVariables.SHELL = "/bin/sh"; };
  launchd.user.agents.yabai = lib.mkIf on { serviceConfig.EnvironmentVariables.SHELL = "/bin/sh"; };

  services.skhd = {
    enable = on;
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
        # Shrink or grow the focused window by 50px: width, or height with ctrl.
        alt + shift - 0x1B : ${resizeScript} -50 w
        alt + shift - 0x18 : ${resizeScript} 50 w
        ctrl + alt + shift - 0x1B : ${resizeScript} -50 h
        ctrl + alt + shift - 0x18 : ${resizeScript} 50 h
      '';
  };
}
