#!/bin/sh
# Fake events: one in progress, one starting soon and two later.
read -r _hello
printf '%s\n' '{"type":"hello","protocol":1,"features":[]}'
{ cat <<'JSON'
{"type":"snapshot","title":"Calendar","rows":[
{"text":"Design review","detail":"now–10:30","detail_tone":"accent","bar_tone":"accent"},
{"text":"Standup","detail":"in 8m–10:45","detail_tone":"warning","bar_tone":"warning"},
{"text":"1:1 with Ana","detail":"14:00–14:30","detail_tone":"muted","bar_tone":"muted"},
{"text":"Focus time","detail":"15:00–17:00","detail_tone":"muted","bar_tone":"muted"}
]}
JSON
} | tr -d '\n'
echo
