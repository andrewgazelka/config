# GUI apps with no good nixpkgs build. `zap` uninstalls anything not listed.
{
  homebrew = {
    enable = true;
    global.autoUpdate = false;
    onActivation = {
      cleanup = "zap";
      autoUpdate = false;
      upgrade = false;
    };

    brews = [ "mas" ]; # masApps installs through it

    casks = [
      "beeper"
      "blender"
      "cardhop"
      "chatgpt"
      "claude"
      "cmux"
      "contexts"
      "cursor"
      "ghostty"
      "google-chrome"
      "granola"
      "helium-browser"
      "jetbrains-toolbox"
      "linear"
      "lm-studio"
      "mullvad-vpn"
      "notion"
      "obs"
      "obsidian"
      "postico"
      "prismlauncher"
      "raycast"
      "realvnc-connect-viewer"
      "screen-studio"
      "setapp"
      "signal"
      "skim"
      "slack"
      "spotify"
      "stremio"
      "superhuman"
      "superwhisper"
      "tableplus"
      "tailscale-app"
      "thebrowsercompany-dia"
      "todoist-app"
      "tor-browser"
      "wireshark-app"
      "zed"
      "zoom"
    ];

    masApps = {
      "Apple Configurator 2" = 1037126344;
      "Apple Developer" = 640199958;
      "Fantastical" = 975937182;
      "Final Cut Pro" = 424389933;
      "Flighty" = 1358823008;
      "GarageBand" = 682658836;
      "iMovie" = 408981434;
      "Keynote" = 361285480;
      "Logic Pro" = 634148309;
      "Microsoft Excel" = 462058435;
      "Microsoft Outlook" = 985367838;
      "Microsoft PowerPoint" = 462062816;
      "Microsoft Word" = 462054704;
      "Numbers" = 361304891;
      "Pages" = 361309726;
      "Portal" = 1436994560;
      "Super Easy Timer" = 1353137878;
      "TestFlight" = 899247664;
      "Things 3" = 904280696;
      "WireGuard" = 1451685025;
      "Xcode" = 497799835;
    };
  };
}
