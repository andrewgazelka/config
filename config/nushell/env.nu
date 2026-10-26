# `session` in shell.cfg, rendered by strata.
source ~/.config/nushell-session-vars.nu

$env.EDITOR = "nvim"
$env.GIT_EDITOR = "nvim"  # a parent (e.g. Claude Code) may export GIT_EDITOR=true
$env.PAGER = "less"
$env.LESS = "-RF --mouse"
$env.DELTA_PAGER = "less -RF --mouse"
$env.FZF_DEFAULT_COMMAND = "fd --type f"
$env._ZO_FZF_OPTS = "+i"
$env.HOMEBREW_NO_ENV_HINTS = 1
$env.RUST_BACKTRACE = "full"
$env.NEXTEST_EXPERIMENTAL_LIBTEST_JSON = 1
$env.CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS = 1
$env.CLAUDE_CODE_NO_FLICKER = 1

$env.PATH = $env.PATH | split row (char esep) | prepend [
    ($env.HOME | path join .local/bin)
    ($env.HOME | path join .cargo/bin)
    ($env.HOME | path join .nix-profile/bin)
    $"/etc/profiles/per-user/($env.USER)/bin"
    /run/wrappers/bin  # NixOS setuid wrappers must precede the system profile
    /run/current-system/sw/bin
    /nix/var/nix/profiles/default/bin
    ($env.HOME | path join .bun/bin)
    ($env.HOME | path join .local/share/npm/bin)
]

if $nu.os-info.name == macos {
    $env.PATH = $env.PATH | prepend [
        ($env.HOME | path join "Library/Application Support/JetBrains/Toolbox/scripts")
        /opt/homebrew/bin
        /opt/homebrew/sbin
        /usr/local/bin
    ]
    $env.HOMEBREW_PREFIX = "/opt/homebrew"
    $env.HOMEBREW_CELLAR = "/opt/homebrew/Cellar"
    $env.HOMEBREW_REPOSITORY = "/opt/homebrew"
    $env.MANPATH = $"/opt/homebrew/share/man:($env.MANPATH? | default '')"
    $env.INFOPATH = $"/opt/homebrew/share/info:($env.INFOPATH? | default '')"
}

# Let builds find libraries from the nix profile.
let nix_lib = $env.HOME | path join .nix-profile/lib
$env.PKG_CONFIG_PATH = $"($nix_lib)/pkgconfig:($env.PKG_CONFIG_PATH? | default '')"
$env.LIBRARY_PATH = $"($nix_lib):($env.LIBRARY_PATH? | default '')"
$env.DYLD_FALLBACK_LIBRARY_PATH = $"($nix_lib):($env.DYLD_FALLBACK_LIBRARY_PATH? | default '')"
