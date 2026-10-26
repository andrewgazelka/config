# Directory jumping: every move lists the new directory.

def show [] { ^eza --icons=auto --group-directories-first --sort=newest }

# Path-like tokens complete directories; bare words complete cwd subdirs plus zoxide hits.
def "nu-complete c" [buffer: string] {
    let token = $buffer | str trim | split row ' ' | last

    if $token =~ '^[~./]' or ($token | str contains '/') {
        let dir = if ($token | str ends-with '/') { $token } else { $token | path dirname }
        let dirs = try {
            ls --short-names ($dir | path expand --no-symlink)
            | where type == dir
            | each {|d| { value: (($dir | path join $d.name) + '/'), description: dir } }
        } catch { [] }
        return { options: { completion_algorithm: prefix, case_sensitive: false }, completions: $dirs }
    }

    # zoxide already ranked its hits, so nushell's own filter and sort are off.
    let lc = $token | str lowercase
    let local = ls --short-names
        | where type == dir and (($lc | is-empty) or ($it.name | str lowercase | str contains $lc))
        | each {|d| { value: ($d.name + '/'), description: dir } }
    let pwd = $env.PWD | path expand
    let zox = if ($token | is-empty) { [] } else {
        # `complete` drains zoxide fully; a >64KB stream inside `try` deadlocks.
        try { ^zoxide query --list -- $token | complete | get stdout | lines } catch { [] }
        | first 30
        | each {|p|
            let shown = if ($p | str starts-with $"($pwd)/") { $p | str replace $"($pwd)/" '' } else if ($p | str starts-with $nu.home-dir) {
                $p | str replace $nu.home-dir '~'
            } else { $p }
            { value: ($p + '/'), display_override: ($shown + '/'), description: zoxide }
        }
        | where display_override not-in ($local | get value)
    }
    { options: { filter: false, sort: false }, completions: ($local | append $zox) }
}

# cd to a path, or else to zoxide's best match for the words.
export def --env c [...query: string@"nu-complete c"] {
    if ($query | is-empty) { cd ~; show; return }
    # Joined so an unquoted dir with spaces still resolves.
    let joined = $query | str join ' ' | path expand
    cd (if ($joined | path type) == dir { $joined } else { ^zoxide query -- ...$query | str trim })
    show
}

export def --env up [] { cd ..; show }
export def --env '...' [] { cd ../..; show }
export def --env mkd [dir: path] { mkdir $dir; c $dir }

# yazi, following its cwd on exit.
export def --env y [...args] {
    let tmp = mktemp -t "yazi-cwd.XXXXXX"
    ^yazi ...$args --cwd-file $tmp
    let cwd = open $tmp
    if $cwd != "" and $cwd != $env.PWD { cd $cwd }
    rm -fp $tmp
}
