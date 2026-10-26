# macOS preferences.
{
  system.defaults = {
    NSGlobalDomain = {
      InitialKeyRepeat = 10; # 150 ms
      KeyRepeat = 2; # 30 ms
      ApplePressAndHoldEnabled = false;
      AppleShowAllExtensions = true;
      AppleICUForce24HourTime = true;
      NSWindowShouldDragOnGesture = true;
      NSWindowResizeTime = 0.001;
      NSNavPanelExpandedStateForSaveMode = true;
      NSNavPanelExpandedStateForSaveMode2 = true;
      PMPrintingExpandedStateForPrint = true;
      PMPrintingExpandedStateForPrint2 = true;
      NSAutomaticDashSubstitutionEnabled = false;
      NSAutomaticPeriodSubstitutionEnabled = false;
      NSAutomaticQuoteSubstitutionEnabled = false;
      "com.apple.trackpad.scaling" = 3.0;
      "com.apple.swipescrolldirection" = false;
    };

    dock = {
      autohide = true;
      mru-spaces = false;
      show-recents = false;
      minimize-to-application = true;
      persistent-apps = [ ];
      persistent-others = [
        {
          folder = {
            path = "/Users/andrewgazelka/Downloads";
            showas = "grid";
            displayas = "folder";
            arrangement = "date-added";
          };
        }
      ];
      # No hot corners.
      wvous-tl-corner = 1;
      wvous-tr-corner = 1;
      wvous-bl-corner = 1;
      wvous-br-corner = 1;
    };

    finder = {
      AppleShowAllExtensions = true;
      ShowPathbar = true;
      ShowStatusBar = true;
      _FXShowPosixPathInTitle = true;
      NewWindowTarget = "Home";
    };

    trackpad = {
      Clicking = false;
      TrackpadRightClick = true;
      TrackpadThreeFingerDrag = false;
    };

    screencapture = {
      type = "png";
      disable-shadow = true;
    };

    # Lock the moment the display sleeps.
    screensaver = {
      askForPassword = true;
      askForPasswordDelay = 0;
    };

    menuExtraClock = {
      ShowSeconds = true;
      Show24Hour = true;
      ShowDate = 1;
      ShowDayOfWeek = true;
    };

    controlcenter.BatteryShowPercentage = true;

    CustomUserPreferences = {
      "com.apple.HIToolbox".AppleFnUsageType = 0; # fn does nothing
      "com.apple.desktopservices" = {
        DSDontWriteNetworkStores = true;
        DSDontWriteUSBStores = true;
      };
    };
  };
}
