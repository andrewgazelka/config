# Show an image inline via the kitty graphics protocol (Ghostty, kitty).
export def display-img [data: binary] {
    print -n $"\e_Ga=T,f=100;($data | encode base64)\e\\"
    print ""
}

# `open x.png` renders the image; `open x.jsonl` parses it.
export def "from png" [] { display-img $in }
export def "from jsonl" [] { from json -o }

# Pull the first JSON object or array out of surrounding text.
export def findjson []: string -> any {
    $in
    | parse --regex '(\{(?:[^{}]|\{[^{}]*\})*\}|\[(?:[^\[\]]|\[[^\[\]]*\])*\])'
    | get capture0.0
    | from json
}

# Digit-group every number in a nested value.
export def format_numbers []: any -> any {
    let data = $in
    match ($data | describe | str replace -r '<.*' '') {
        "int" | "float" => ($data | into string --group-digits)
        "list" | "table" => ($data | each { format_numbers })
        "record" => ($data | items {|k, v| {$k: ($v | format_numbers)} } | into record)
        _ => $data
    }
}

# Apple Core Data timestamps (nanoseconds since 2001) as datetimes.
export def "into apple-timestamp" [--timezone (-t): string = 'America/Los_Angeles'] {
    $in + 978_307_200_000_000_000 | into datetime | date to-timezone $timezone
}

# Replace a Postgres table's rows with piped records (same schema) in one transaction.
export def replace-table [connection: string, table: string]: list -> any {
    $in | to json | ^psql $connection -c $"
BEGIN;
TRUNCATE ($table);
INSERT INTO ($table) SELECT * FROM json_populate_recordset\(null::($table), stdin::json\);
COMMIT;"
}

# Ancestors of a process, from it up to the root.
export def ptree [pid: int] {
    let procs = ps
    mut current = $pid
    mut chain = []
    while $current != 0 {
        let proc = $procs | where pid == $current | get -o 0
        if $proc == null { break }
        $chain ++= [$proc]
        $current = $proc.ppid
    }
    $chain
}

# All descendants of a process.
export def descendants [pid: int] {
    let procs = ps
    mut frontier = [$pid]
    mut found = []
    while ($frontier | is-not-empty) {
        let parents = $frontier
        let children = $procs | where ppid in $parents
        $found ++= $children
        $frontier = $children | get pid
    }
    $found
}

# Copy a file to the clipboard like Cmd-C in Finder.
export def cpfile [path: path] {
    let path = $path | path expand
    let script = r#'use AppleScript version "2.4"
use framework "Foundation"
use framework "AppKit"

set theURL to current application's |NSURL|'s fileURLWithPath:"__PATH__"
set theClip to current application's NSPasteboard's generalPasteboard()
theClip's clearContents()
theClip's writeObjects:{theURL}
theClip's setString:"__FILENAME__" forType:(current application's NSPasteboardTypeString)
'#
    ^osascript -e ($script | str replace __PATH__ $path | str replace __FILENAME__ ($path | path basename)) | ignore
}

# launchctl only lists loaded jobs; this finds installed ones from their plists.
export def "launchctl installed" [] {
    [
        [scope kind dir];
        [user agent ($env.HOME | path join Library/LaunchAgents)]
        [system agent /Library/LaunchAgents]
        [system daemon /Library/LaunchDaemons]
    ]
    | each {|loc|
        glob ($loc.dir | path join "*.plist") | par-each {|path|
            { label: (^plutil -convert json -o - $path | from json | get -o Label), scope: $loc.scope, kind: $loc.kind, path: $path }
        }
    }
    | flatten
    | where label != null  # empty vendor placeholders
    | sort-by label
}

# Publish an HTML file as a private gist; copies and returns a rendered-preview link.
export def share [path: path] {
    let name = $path | path basename
    let gist = { public: false, files: { $name: { content: (open --raw $path) } } }
        | to json
        | ^gh api gists --input -
        | from json
    let url = $"https://gistpreview.github.io/?($gist.id)/($name)"
    $url | ^pbcopy
    $url
}

export def yt-captions [url: string, --lang (-l): string = "en"] {
    ^yt-dlp --write-auto-sub --sub-lang $lang --skip-download -o "%(title)s" $url
}
