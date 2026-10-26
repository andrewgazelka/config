# config

My Mac in one repo: [nix-darwin](https://github.com/nix-darwin/nix-darwin)
for the system, [home-manager](https://github.com/nix-community/home-manager)
for packages and background agents, and every dotfile written in strata, a
small config language whose engine lives here too.

```
flake.nix   the system (darwinConfigurations.hydra), strata, checks
darwin/     nix, users, homebrew, macOS defaults, DNS blocklist
home/       packages, shells, launchd agents, activation scripts
config/     the dotfiles: *.cfg in strata, plus plain trees (nvim, nushell, agents)
strata/     the engine: Rust, its stdlib of app modules, tests
```

## Use

```sh
darwin-rebuild switch --flake .
nix flake check   # secret scan, clippy, tests, fmt
nix fmt
```

Dotfiles need no switch. A launchd agent (`home/strata.nix`) runs
`strata watch`, which applies `config/` at login and again on every save.
By hand, from the repo root:

```sh
strata plan        # what apply would change, reading ~ only
strata apply
strata rollback    # back to the previous generation
strata explain 'zed.font[size]'
```

The checkout is expected at `/Volumes/Projects/nix` (`repo` in
`home/strata.nix`; `config/cursor.cfg` names it for nixd). A private repo
at `private/` (gitignored) adds its strata files from `private/config/`:
`--config` is repeatable, `--config-optional` names a dir that is skipped
when absent, and all dirs merge as one config.

Another flake can build on this one: `darwinModules.default` is the whole
system (home-manager included), so `darwinSystem { modules = [
config.darwinModules.default ./more.nix ]; }` adds to it, and home modules
join through `home-manager.users.<name>.imports`.

## strata

Braces open a scope, a colon sets one value. `name { ... }` is an entity
that any file may reopen, merged key by key; `name: value` is a component,
replaced whole. App keys sit on the app entity verbatim, quoted when needed.
This is Flecs script's split: `name {` never means `name: {`, which strata
refuses, so record literals appear only in expressions (lists, arguments,
rules). `name : base { ... }` builds on prefabs.

```
aerospace {
    config-file: "~/.aerospace.toml"
    accordion-padding: 100
    start-at-login: false
    bindings { "alt-tab": "workspace-back-and-forth" }
    window-rules { "com.linear": "tiling" }
}

zed : typography {
    font { size: 14 }
    vim_mode: true
    terminal { shell { program: "nu" } }
}
```

A module (`strata/stdlib/<app>.cfg`) renders its entity's subtree to the
app's format, children as nested objects and components as keys, leaving out
the fields it declares and translates itself (`config-file`, `bindings`,
`window-rules`, `font`, ...). `core.cfg`'s components (`file`, `dirs`,
`texts`) are global; inside `zed` only zed's module applies. Every name
strata or the stdlib defines is kebab-case, so subtraction needs spaces.

Claude Code's keys are typed key by key by
`stdlib/schemas/claude-settings.cfg`, generated with `strata schema import
<url or file> --name <x>` (the header names the source and its blake3). An
unknown key in a closed object, or a wrong type, fails with file:line:col.

Two core rules let a second layer add to a file or directory instead of
replacing it. `dirs."<target>" { public: entries("dir") }` links each
top-level entry of `dir` to `<target>/<name>`, so the target stays a real
directory an app can add its own entries to; each layer adds its own
component (`private: ...`) and entries merge by name.
`texts."<target>" { "10-x": read("x.md") }` joins its components in name
order into one file. `read()` is a file's text, `entries()` a directory's
entries as `source()`/`tree()` values, and `link(path)` and `copy(file)` make
a `file` a symlink or a copy.

Modules in `strata/stdlib` turn app entities like `zed` into
`files."<target>"` entities, each holding one `file`: JSON, TOML, INI, text,
a file or tree from `config/`, or a link to a Nix store path. Values merge by
level, and two different values at one level are an error naming both. Each
file is stored by its blake3 hash under `~/.local/state/strata`, a generation
is the set of files, named by one hash, and `apply` switches `~` to it with
one rename per file: a symlink into the store, or a copy for apps that
rewrite their own settings (Zed, Cursor, Claude Code). Evaluation is
incremental, so a save re-renders only what it touches.

Store paths (delta, gh, ...) come from `config/nix/facts.nix`, evaluated
against the nixpkgs in `flake.lock`. The agent gets the system's table;
`strata nix-facts` caches one for running strata by hand.

The golden tests (`strata/tests/golden`) compare against output recorded
from the Nickel config strata replaced. The crash test kills `apply` at each
crash point, damages what a power cut could, and checks the next run
converges. `cargo bench --bench latency` times cold runs and edits.

## First switch

Coming from a home-manager that wrote these dotfiles itself:

```sh
nix run .#strata -- nix-facts
nix run .#strata -- apply --adopt   # old files go to ~/.local/state/strata/backups
darwin-rebuild switch --flake .
nix run .#strata -- plan            # every target: none
```
