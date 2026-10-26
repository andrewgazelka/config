use ($nu.default-config-dir | path join functions) *
use ($nu.default-config-dir | path join completions.nu) *

# starship draws the prompt; drop nushell's vi-mode markers.
$env.PROMPT_INDICATOR_VI_INSERT = ""
$env.PROMPT_INDICATOR_VI_NORMAL = ""

# LS_COLORS follows the macOS appearance.
if (which vivid | is-not-empty) {
    let dark = $nu.os-info.name != macos or (^defaults read -g AppleInterfaceStyle | complete).stdout =~ Dark
    $env.LS_COLORS = ^vivid generate (if $dark { "snazzy" } else { "one-light" })
}

alias vim = nvim
alias cat = bat
alias man = /usr/bin/man
alias tl = tail -n +1 -f

# Abbreviations expand on space; enter skips ones with a `%` cursor marker.
let abbreviations = open ($nu.default-config-dir | path join abbreviations.nuon)
let anywhere_abbreviations = { LJ: "--log-format internal-json", H: "--help" }

let abbr_menu = {|name: string, skip_placeholders: bool|
    {
        name: $name
        only_buffer_difference: false
        marker: none
        type: { layout: columnar, columns: 1 }
        style: {}
        source: {|buffer|
            let value = if $skip_placeholders {
                abbr expand $buffer $abbreviations $anywhere_abbreviations --skip-placeholders
            } else {
                abbr expand $buffer $abbreviations $anywhere_abbreviations
            }
            { value: $value }
        }
    }
}

# nix's NIX_GET_COMPLETIONS protocol: a type line, then value<TAB>description lines.
# nom is a drop-in for nix build/develop/shell, so it shares them.
let nix_completer = {|spans|
    let out = with-env { NIX_GET_COMPLETIONS: (($spans | length) - 1 | into string) } {
        ^nix ...($spans | skip 1) | complete
    }
    if $out.exit_code != 0 { return [] }
    $out.stdout | lines | skip 1 | where $it != "" | split column "\t" value description
}

$env.config = {
    show_banner: false
    edit_mode: vi
    cursor_shape: { vi_insert: line, vi_normal: block }
    use_kitty_protocol: true
    footer_mode: never
    shell_integration: { osc2: false, osc633: false }  # leave tab titles to the terminal
    history: { file_format: sqlite, sync_on_enter: false, isolation: true }
    completions: {
        algorithm: fuzzy
        case_sensitive: true
        external: {
            completer: {|ctx|
                match $ctx.command.0 { "nix" | "nom" => (do $nix_completer $ctx.command), _ => null }
            }
        }
    }
    table: {
        mode: none
        index_mode: never
        header_on_separator: true
        padding: { left: 0, right: 1 }
        missing_value_symbol: ""
        abbreviated_row_count: 3
        trim: { methodology: wrapping, wrapping_try_keep_words: false }
    }
    color_config: {
        separator: dark_gray
        header: "#999937"
        row_index: dark_gray
        filesize: "#89dceb"
        datetime: purple
        int: default
        float: default
        duration: default
        # Log levels and `glc` commit types.
        string: {||
            match $in {
                "TRACE" | "Test" | "Build" | "CI" => "cyan"
                "DEBUG" | "started" | "Docs" => "blue"
                "INFO" | "ok" | "Feature" => "green"
                "WARN" | "Refactor" | "Perf" => "yellow"
                "ERROR" | "Fix" => "red"
                "Style" => "magenta"
                "Chore" => "dark_gray"
                _ => "default"
            }
        }
    }
    # Catppuccin Mocha, with fg and bg on every style so it reads on light and dark terminals.
    explore: {
        try: { reactive: true }
        status_bar_background: { fg: "#cdd6f4", bg: "#313244" }
        status_bar_text: { fg: "#cdd6f4", bg: "#313244" }
        command_bar_text: { fg: "#cdd6f4", bg: "#1e1e2e" }
        highlight: { fg: "#1e1e2e", bg: "#f9e2af" }
        status: {
            error: { fg: "#1e1e2e", bg: "#f38ba8" }
            warn: { fg: "#1e1e2e", bg: "#f9e2af" }
            info: { fg: "#1e1e2e", bg: "#89b4fa" }
        }
        selected_cell: { fg: "#1e1e2e", bg: "#f9e2af" }
        selected_row: { fg: "#cdd6f4", bg: "#313244" }
        selected_column: { fg: "#cdd6f4", bg: "#313244" }
        table: {
            split_line: { fg: "#585b70", bg: "#1e1e2e" }
            cursor_row: { fg: "#cdd6f4", bg: "#313244" }
            cursor_column: { fg: "#cdd6f4", bg: "#313244" }
            cursor_cell: { fg: "#1e1e2e", bg: "#f9e2af" }
            line_head_top: { fg: "#585b70", bg: "#1e1e2e" }
            line_head_bottom: { fg: "#585b70", bg: "#1e1e2e" }
            line_shift: { fg: "#585b70", bg: "#1e1e2e" }
            line_index: { fg: "#585b70", bg: "#1e1e2e" }
            show_head: true
            show_index: true
        }
    }
    hooks: {
        display_output: { render-output }
        env_change: {
            # direnv has no nushell hook of its own here.
            PWD: [{||
                if (which direnv | is-not-empty) {
                    ^direnv export json | from json | default {} | load-env
                    if ($env.PATH | describe) == string { $env.PATH = $env.PATH | split row (char esep) }
                }
            }]
        }
        pre_prompt: [{||
            # A Ghostty shell orphaned to launchd lingers forever; `exit` is ignored in hooks.
            if "GHOSTTY_RESOURCES_DIR" in $env and (ps | where pid == $nu.pid | get -o 0.ppid) == 1 {
                ^/bin/kill $nu.pid
            }
            # Bell so tmux flags windows whose command finished.
            if "TMUX" in $env { print -n (char bel) }
        }]
    }
}

$env.config.menus ++= [(do $abbr_menu abbr_menu false) (do $abbr_menu abbr_menu_enter true)]

const all_modes = [emacs vi_normal vi_insert]
$env.config.keybindings ++= [
    { name: abbr_space, modifier: none, keycode: space, mode: $all_modes
      event: [{ send: menu, name: abbr_menu } { edit: insertchar, value: ' ' }] }
    { name: abbr_enter, modifier: none, keycode: enter, mode: $all_modes
      event: [{ send: menu, name: abbr_menu_enter } { send: enter }] }
    # Jump to an expanded abbreviation's `%` marker.
    { name: abbr_placeholder, modifier: control, keycode: space, mode: $all_modes
      event: { send: executehostcommand, cmd: "
        let line = commandline
        let pos = $line | str index-of '%'
        if $pos >= 0 { commandline edit --replace ($line | str replace '%' ''); commandline set-cursor $pos }
      " } }
    # Also clears scrollback, like Cmd-K.
    { name: clear_scrollback, modifier: control, keycode: char_l, mode: $all_modes, event: { send: clearscrollback } }
    { name: delete_word_left, modifier: alt, keycode: backspace, mode: $all_modes, event: { edit: backspaceword } }
    { name: delete_to_start, modifier: control, keycode: char_u, mode: $all_modes, event: { edit: cutfromstart } }
    # Accept the history hint, like fish.
    { name: accept_hint, modifier: control, keycode: char_f, mode: $all_modes, event: { send: historyhintcomplete } }
]

# Autopairs bind in insert modes only, so vi-normal motions stay intact.
$env.config.keybindings ++= (
    [[key cmd]; [char_u28 'autopair-open "(" ")"'] [char_u29 'autopair-close ")"']
                [char_u5b 'autopair-open "[" "]"'] [char_u5d 'autopair-close "]"']
                [char_u7b 'autopair-open "{" "}"'] [char_u7d 'autopair-close "}"']
                [char_u22 "autopair-quote '\"'"]]
    | each {|k| {
        name: $"autopair_($k.key)", modifier: none, keycode: $k.key, mode: [emacs vi_insert]
        event: { send: executehostcommand, cmd: $k.cmd }
    } }
)
