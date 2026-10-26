# Flake inputs by lock age, unlocked ones first.
export def locks [] {
    let rows = ^nix flake metadata --json
        | from json
        | get locks.nodes
        | items {|node, data|
            let locked = $data.locked? | default {}
            {
                node: $node
                lastModified: ($locked.lastModified? | if $in != null { into datetime -f '%s' })
                rev: $locked.rev?
            }
        }
    ($rows | where lastModified == null) ++ ($rows | where lastModified != null | sort-by lastModified)
}

# home-manager generations, newest first.
export def gens [] {
    let state = $env.XDG_STATE_HOME? | default ($env.HOME | path join .local/state)
    let dir = [($state | path join nix/profiles) $"/nix/var/nix/profiles/per-user/($env.USER)"]
        | where { $in | path join home-manager | path exists }
        | get -o 0
    if $dir == null { return [] }
    let current = ^readlink ($dir | path join home-manager)

    glob ($dir | path join "home-manager-*-link")
    | each {|link| {
        id: ($link | path basename | parse "home-manager-{id}-link" | get 0.id | into int)
        datetime: (ls -lD $link | get 0.modified)
        current: (($link | path basename) == $current)
        store_path: (^readlink $link)
    } }
    | sort-by -r id
}

# Follow every symlink on the way to a path: one row per hop, per path component.
export def symlink-trail [path: path = "."] {
    let parts = $path | path expand --no-symlink | path split
    1..($parts | length)
    | each {|n|
        let component = $parts | first $n | path join
        mut rows = []
        mut current = $component
        while ($current not-in ($rows | get -o path | default [])) {
            let entry = try { ls -lD $current | get 0 } catch { break }
            if $entry.type != symlink { break }
            let resolved = if ($entry.target | str starts-with '/') { $entry.target } else {
                $current | path dirname | path join $entry.target
            } | path expand --no-symlink
            $rows ++= [{ component: $component, hop: ($rows | length), path: $current, target: $entry.target, resolved: $resolved }]
            $current = $resolved
        }
        $rows
    }
    | flatten
    | enumerate
    | each {|e| $e.item | insert step $e.index }
    | select step component hop path target resolved
}
