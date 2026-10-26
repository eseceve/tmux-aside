#!/bin/sh
# Fake pull requests covering every state: checks running, failing, conflicting, changes
# requested, ready, waiting for review and draft.
read -r _hello
printf '%s\n' '{"type":"hello","protocol":1,"features":[]}'
{ cat <<'JSON'
{"type":"snapshot","title":"Pull requests","rows":[
{"text":"feat(auth): split the middleware","detail":"#1342","detail_tone":"accent","bar_tone":"accent","group":"api","lines":["+120 -8 · 2 unresolved · 0/2 approvals"]},
{"text":"fix(db): read the pool size from env","detail":"#1339","detail_tone":"danger","bar_tone":"danger","group":"api","lines":["+12 -4 · 1/2 approvals"]},
{"text":"docs: getting started","detail":"#1338","detail_tone":"success","bar_tone":"success","group":"docs","lines":["+310 -12 · 1/1 approvals"]},
{"text":"docs: plugin guide","detail":"#1330","detail_tone":"muted","bar_tone":"muted","group":"docs","lines":["+64 -0 · 0 approvals"]},
{"text":"fix(cart): keep totals in sync","detail":"#1340","detail_tone":"warning","bar_tone":"warning","group":"web","lines":["+42 -3 · 3 unresolved · 0/1 approvals"]},
{"text":"feat(nav): sticky header","detail":"#1335","detail_tone":"danger","bar_tone":"danger","group":"web","lines":["+88 -20 · 1/1 approvals"]}
]}
JSON
} | tr -d '\n'
echo
