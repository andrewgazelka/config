# Fish-style abbreviations: expand the word before the cursor in place.
# `commands` expand only at command position, `anywhere` at any position.
# --skip-placeholders leaves expansions containing a `%` cursor marker alone.
export def expand [
    buffer: string
    commands: record
    anywhere: record
    --skip-placeholders
]: nothing -> string {
    # The segment after the last separator is the command being typed.
    let segment = $buffer | split row --regex '[|;&{(]' | last | str trim --left
    let words = $segment | split row ' '
    let word = $words | last

    let expansion = if ($words | length) == 1 and $word in $commands {
        $commands | get $word
    } else {
        $anywhere | get -o $word
    }

    if $expansion == null or ($skip_placeholders and '%' in $expansion) {
        return $buffer
    }
    ($buffer | str substring 0..<(($buffer | str length) - ($word | str length))) + $expansion
}
