#!/bin/sh
# Starts a separate tmux server showing the sidebar with fake data, for screenshots.
# Usage: examples/demo/run.sh [path/to/tmux-aside]; then attach with the command it prints.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin=${1:-$(command -v tmux-aside)}
socket=tmux-aside-demo
home=$(mktemp -d)
mkdir -p "$home/config/tmux-aside"
sed "s|@DEMO@|$here|g" "$here/config.yaml" > "$home/config/tmux-aside/config.yaml"

tmux -L "$socket" run-shell "$bin off" 2>/dev/null || true
tmux -L "$socket" kill-server 2>/dev/null || true
XDG_CONFIG_HOME="$home/config" XDG_STATE_HOME="$home/state" \
  tmux -L "$socket" -f "$here/tmux.conf" new-session -d -s demo -n api-refactor -x 160 -y 50
for window in checkout docs-site flaky-tests mobile; do
  tmux -L "$socket" new-window -d -t demo -n "$window"
done
tmux -L "$socket" run-shell "$bin on --width 40"
echo "attach with: tmux -L $socket attach -t demo"
echo "stop with:   tmux -L $socket kill-server"
