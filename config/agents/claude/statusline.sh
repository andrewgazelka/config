#!/bin/bash
# Claude Code statusline: model | cwd | PID
input=$(cat)
model=$(echo "$input" | jq -r '.model.display_name // .model.id // "?"')
cwd=$(echo "$input" | jq -r '.workspace.current_dir // .cwd // "?"')
# Statusline is spawned by the Claude Code process, so $PPID is Claude's PID
echo "$model | ${cwd/#$HOME/~} | PID:$PPID"
