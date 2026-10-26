#!/bin/sh
# Fake pull requests waiting for your review.
read -r _hello
printf '%s\n' '{"type":"hello","protocol":1,"features":[]}'
{ cat <<'JSON'
{"type":"snapshot","title":"Review requested","rows":[
{"text":"feat(push): quiet hours","detail":"#221","detail_tone":"muted","bar_tone":"muted","group":"mobile","lines":["+512 -40 · @octocat · 3h"]},
{"text":"fix(login): keep the session","detail":"#219","detail_tone":"success","bar_tone":"success","group":"mobile","lines":["+18 -6 · @hubot · 1d"]}
]}
JSON
} | tr -d '\n'
echo
