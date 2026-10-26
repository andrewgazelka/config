# Recursive `strings`: also scan every existing file a string names, breadth-first.
# One row per string: source file, depth, line, text, and the file it names (if any).
export def main [
    path?: path  # Initial file; otherwise a path or list of paths from stdin.
    --max-depth (-d): int = 3  # Initial files are depth zero.
    --threads (-t): int = 8
] {
    let input = $in
    let roots = if $path != null { [$path] } else if $input == null { [] } else { [$input] | flatten }
        | each { into string | path expand }
        | uniq
    if ($roots | is-empty) { error make { msg: "rstrings needs at least one file" } }
    let bad = $roots | where { ($in | path type) != file }
    if ($bad | is-not-empty) { error make { msg: $"not files: ($bad | str join ', ')" } }

    mut frontier = $roots
    mut seen = $roots
    mut rows = []
    for depth in 0..$max_depth {
        if ($frontier | is-empty) { break }
        let found = $frontier
            | par-each --keep-order --threads $threads {|source|
                do --capture-errors { ^strings -- $source }
                | lines
                | enumerate
                | each { { source: $source, depth: $depth, line: ($in.index + 1), text: $in.item } }
            }
            | flatten
        let names = $found
            | get text
            | uniq
            | each {|text|
                let p = try { $text | path expand } catch { "" }
                { text: $text, discovered_path: (if $text != "" and ($p | path type) == file { $p }) }
            }
        let found = $found | join --left $names text
        $rows ++= $found
        let known = $seen
        $frontier = $found | get discovered_path | compact | uniq | where $it not-in $known
        $seen ++= $frontier
    }
    $rows
}
