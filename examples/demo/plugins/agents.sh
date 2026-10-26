#!/bin/sh
# Fake agents covering every status: working, waiting, done, stalled and idle.
read -r _hello
id() { tmux display-message -p -t "demo:$1" '#{window_id}' 2>/dev/null; }
printf '%s\n' '{"type":"hello","protocol":1,"features":[]}'
{ cat <<JSON
{"type":"snapshot","title":"Agents","rows":[
{"text":"checkout","detail":"2:14","detail_tone":"muted","bar_tone":"warning","window":"$(id checkout)","lines":["web · +42 -3 · fix/cart-totals"]},
{"text":"api-refactor","detail":"spinner","detail_tone":"accent","bar_tone":"accent","window":"$(id api-refactor)","lines":["api · +120 -8 · feat/split-auth","Split the auth middleware"]},
{"text":"flaky-tests","detail":"stalled","detail_tone":"danger","bar_tone":"danger","window":"$(id flaky-tests)","lines":["api · +5 -1 · fix/payment-retry"]},
{"text":"docs-site","detail":"#1338 ✓","detail_tone":"success","bar_tone":"success","window":"$(id docs-site)","lines":["docs · +310 -12 · docs/guide"]},
{"text":"mobile","detail":"#221 ×","detail_tone":"danger","bar_tone":"muted","window":"$(id mobile)","lines":["mobile · main"]}
]}
JSON
} | tr -d '\n'
echo
