# Completions nushell lacks. Externs declare only the completed flags; any other
# flag passes through untouched.

# cargo: workspace packages and targets from `cargo metadata`.
def cargo-targets [kind?: string] {
    let meta = ^cargo metadata --format-version 1 --no-deps | from json
    let members = $meta.packages | where id in $meta.workspace_members
    if $kind == null { $members | get name } else {
        $members | get targets | flatten | where $kind in $it.kind | get name
    }
}
def "nu-complete cargo packages" [] { cargo-targets }
def "nu-complete cargo bins" [] { cargo-targets bin }
def "nu-complete cargo examples" [] { cargo-targets example }
def "nu-complete cargo tests" [] { cargo-targets test }

export extern "cargo run" [
    --package (-p): string@"nu-complete cargo packages"
    --bin: string@"nu-complete cargo bins"
    --example: string@"nu-complete cargo examples"
    ...args
]
export extern "cargo build" [
    --package (-p): string@"nu-complete cargo packages"
    --bin: string@"nu-complete cargo bins"
    --example: string@"nu-complete cargo examples"
    ...args
]
export extern "cargo test" [
    --package (-p): string@"nu-complete cargo packages"
    --test: string@"nu-complete cargo tests"
    ...args
]
export extern "cargo nextest run" [
    --package (-p): string@"nu-complete cargo packages"
    --test: string@"nu-complete cargo tests"
    ...args
]
export extern "cargo check" [--package (-p): string@"nu-complete cargo packages", ...args]
export extern "cargo clippy" [--package (-p): string@"nu-complete cargo packages", ...args]
export extern "cargo doc" [--package (-p): string@"nu-complete cargo packages", ...args]
export extern "cargo add" [--package (-p): string@"nu-complete cargo packages", ...args]

# tmux: session names.
def "nu-complete tmux sessions" [] {
    ^tmux list-sessions -F "#{session_name}\t#{session_windows} windows#{?session_attached, attached,}"
    | complete
    | get stdout
    | lines
    | split column "\t" value description
}
export extern "tmux attach" [-t: string@"nu-complete tmux sessions", ...args]
export extern "tmux attach-session" [-t: string@"nu-complete tmux sessions", ...args]
export extern "tmux kill-session" [-t: string@"nu-complete tmux sessions", ...args]

# typst: .typ files.
def "nu-complete typ-files" [] { ^fd --type f -e typ | lines }
export extern "typst compile" [input: string@"nu-complete typ-files", ...args]
export extern "typst watch" [input: string@"nu-complete typ-files", ...args]

# Compile and open.
export def tc [path: string@"nu-complete typ-files", ...rest: string] {
    ^typst compile $path --open ...$rest
}
