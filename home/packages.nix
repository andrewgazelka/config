{ inputs, pkgs, ... }:
let
  inherit (pkgs.stdenv.hostPlatform) system;
in
{
  home.packages = with pkgs; [
    # Core
    coreutils
    moreutils
    curl
    wget
    rsync
    tree
    unzip
    zstd
    watch
    viddy
    htop
    btop
    duf
    dust
    dua
    fd
    ripgrep
    ripgrep-all
    ast-grep
    jq
    dasel
    sd
    glow
    hyperfine
    tokei
    moor
    croc
    age
    socat
    websocat
    mosh
    mtr
    doggo

    # Git
    git
    git-lfs
    delta
    gh
    gitoxide
    lazygit
    difftastic
    mergiraf

    # Editors
    neovim
    tree-sitter

    # Languages
    nodejs_24
    bun
    python313
    uv
    ruff
    pyright
    go
    zig
    cmake
    pkg-config
    just
    protobuf
    gdb

    # Nix
    nil
    nixd
    nixfmt
    nix-tree
    nvd
    nix-index

    # Cloud
    awscli2
    google-cloud-sdk
    cloudflared
    pulumi

    # Data
    postgresql_18
    pgcli
    usql
    redis

    # Containers
    dive
    skopeo
    crane

    # Media and documents
    ffmpeg
    imagemagick
    yt-dlp-light
    poppler-utils
    typst
    tinymist

    # macOS
    _1password-cli
    inputs.lifelog.packages.${system}.lifelog
    inputs.notify.packages.${system}.default
  ];
}
