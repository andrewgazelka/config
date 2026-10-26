# display_output hook: big results open in `explore`, small ones print as a table.

def is-big [] {
    let data = $in
    let size = try { $data | to json -r | str length } catch { 0 }
    let rows = try { $data | length } catch { 0 }
    let fields = try { $data | columns | length } catch { 0 }
    let first = try { $data | first | describe } catch { "" }
    let scalar_list = ($data | describe) =~ '^list' and not ($first =~ '^record')
    $size > 2000 or (not $scalar_list and ($rows > 6 or $fields > 6))
}

# Integer fields named *time* become datetimes, guessing the unit from the digit count.
def unix-time [field: string] {
    let value = $in
    let text = $value | into string
    if not ($field | str lowercase | str contains "time") or not ($text =~ '^-?\d+$') {
        return $value
    }
    let digits = $text | str trim --left --char '-' | str length
    try {
        if $digits >= 18 { $value | into datetime } else if $digits >= 13 {
            $value * 1_000_000 | into datetime
        } else if $digits >= 10 { $value | into datetime -f '%s' } else { $value }
    } catch { $value }
}

def normalize [field: string = ""] {
    let value = $in
    match ($value | describe | str replace -r '<.*' '') {
        "record" => ($value | items {|k, v| {$k: ($v | normalize $k)} } | into record)
        "list" | "table" => ($value | each { normalize $field })
        "nothing" => $value
        _ => ($value | unix-time $field)
    }
}

export def render-output [] {
    let data = $in | normalize
    if $nu.is-interactive and ($data | is-big) {
        # Whatever was peeked at in explore lands on the clipboard.
        let peek = $data | explore -ip
        if (which pbcopy | is-not-empty) {
            if ($peek | describe) == "string" { $peek } else { $peek | to nuon } | ^pbcopy
        }
        $peek
    } else if (try { (term size).columns } catch { 0 }) >= 100 {
        $data | table -e
    } else {
        $data | table
    }
}
