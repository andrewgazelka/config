# IDE-style autopairs, bound to keys in config.nu. An opener inserts the pair
# (only before whitespace, a closer or end of line); a closer types over its twin.

def next-char [] {
    let cur = commandline get-cursor
    commandline | str substring $cur..$cur
}

def pairable [] { (next-char) in ["" ")" "]" "}" " " (char tab)] }

export def autopair-open [open: string, close: string] {
    let cur = commandline get-cursor
    if (pairable) {
        commandline edit --insert ($open + $close)
        commandline set-cursor ($cur + 1)
    } else {
        commandline edit --insert $open
    }
}

export def autopair-close [close: string] {
    if (next-char) == $close {
        commandline set-cursor ((commandline get-cursor) + 1)
    } else {
        commandline edit --insert $close
    }
}

# Quotes open and close with the same key.
export def autopair-quote [quote: string] {
    if (next-char) == $quote { autopair-close $quote } else { autopair-open $quote $quote }
}
