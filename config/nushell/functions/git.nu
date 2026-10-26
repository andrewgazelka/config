# Default branch from origin/HEAD (no network), else the GitHub API.
export def git_default_branch [] {
    let local = do { ^git rev-parse --abbrev-ref origin/HEAD } | complete
    if $local.exit_code == 0 {
        $local.stdout | str trim | str replace "origin/" ""
    } else {
        ^gh api repos/{owner}/{repo} --jq '.default_branch' | str trim
    }
}

# Commit log as a table.
export def commits [] {
    ^git log --format="%H%x00%aN%x00%aE%x00%aI%x00%s%x00"
    | split row (char nul)
    | drop
    | chunks 5
    | each {|r| {
        hash: ($r.0 | str trim)
        author: $r.1
        email: $r.2
        authored_at: ($r.3 | into datetime)
        subject: $r.4
    } }
}

# Recent commits grouped by conventional-commit type (colored via color_config).
export def glc [--limit: int = 10] {
    const labels = {
        feat: Feature, fix: Fix, docs: Docs, style: Style, refactor: Refactor
        perf: Perf, test: Test, build: Build, ci: CI, chore: Chore
    }
    ^git log --format=%s -n $limit
    | lines
    | each {|subject|
        let m = $subject | parse --regex '^(?<type>[a-z]+)(?:\((?<scope>[^)]+)\))?!?: (?<message>.+)$'
        if ($m | is-empty) {
            { type: Other, scope: "", message: $subject }
        } else {
            let m = $m.0
            { type: ($labels | get -o $m.type | default Other), scope: ($m.scope | default ""), message: $m.message }
        }
    }
}

# Files with their last commit author and time, oldest first.
export def lb [] {
    ls | each {|f|
        let info = ^git log -1 --format="%an%x00%aI" -- $f.name | complete
        if $info.exit_code == 0 and ($info.stdout | is-not-empty) {
            let parts = $info.stdout | str trim | split row (char nul)
            { when: ($parts.1 | into datetime), name: $f.name, author: $parts.0 }
        } else {
            { when: null, name: $f.name, author: "untracked" }
        }
    } | sort-by when
}

# `git worktree list` as a table (from --porcelain, the only stable format).
export def "git worktrees" [] {
    ^git worktree list --porcelain
    | split row "\n\n"
    | where ($it | str trim | is-not-empty)
    | each {|block|
        let e = $block | lines | where $it != "" | reduce --fold {} {|line, acc|
            let kv = $line | split row --number 2 " "
            # Valueless keys (`bare`, `detached`) become flags.
            $acc | insert $kv.0 ($kv.1? | default true)
        }
        {
            name: ($e.worktree | path basename)
            path: ($e.worktree | str replace $env.HOME "~")
            head: ($e.HEAD | str substring 0..<8)
            branch: ($e.branch? | default "" | str replace "refs/heads/" "")
            detached: ($e.detached? | default false)
            prunable: ($e.prunable? != null)
        }
    }
}

def "nu-complete git-branches" [] {
    let remote = ^git for-each-ref '--format=%(refname:short)' refs/remotes/
        | lines
        | each { str replace --regex '^[^/]+/' '' }
        | where $it != "HEAD"
    ^git for-each-ref '--format=%(refname:short)' refs/heads/ | lines | append $remote | uniq
}

# Switch to a branch, or cd into its worktree if it has one; then pull.
export def --env gw [branch: string@"nu-complete git-branches", ...rest: string] {
    let tree = try { git worktrees | where branch == $branch | get -o 0.path } catch { null }
    if $tree == null {
        ^git switch $branch ...$rest
    } else {
        cd ($tree | path expand)
    }
    ^git pull --rebase
}

def "nu-complete git-changed" [] {
    (^git diff --name-only) + (^git diff --cached --name-only) + (^git ls-files --others --exclude-standard)
    | lines
    | uniq
}

# nvim, completing changed and untracked files.
export def vd [path?: string@"nu-complete git-changed", ...rest: string] {
    if $path == null { ^nvim } else { ^nvim $path ...$rest }
}

# Poll CI for a commit until a run fails or all succeed.
export def watch-gh-runs [--commit: string, --interval: duration = 10sec] {
    let sha = $commit | default (^git rev-parse HEAD | str trim)
    loop {
        let runs = ^gh run list --commit $sha --json databaseId,status,conclusion,name | from json
        let failed = $runs | any { $in.conclusion == "failure" }
        let passed = $runs | all { $in.status == "completed" and $in.conclusion == "success" }
        if ($runs | is-not-empty) and ($failed or $passed) { return $runs }
        sleep $interval
    }
}
