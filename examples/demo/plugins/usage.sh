#!/bin/sh
# Fake plan usage: below 70%, from 70% and from 90%.
read -r _hello
printf '%s\n' '{"type":"hello","protocol":1,"features":[]}'
{ cat <<'JSON'
{"type":"snapshot","title":"Usage","rows":[
{"text":"5 hours","detail":"22% · 3h16m","detail_tone":"muted","group":"Claude","meter":22,"meter_tone":"success"},
{"text":"Weekly ","detail":"74% · 2d4h ","detail_tone":"muted","group":"Claude","meter":74,"meter_tone":"warning"},
{"text":"Weekly ","detail":"93% · 6h5m ","detail_tone":"muted","group":"Codex","meter":93,"meter_tone":"danger"}
]}
JSON
} | tr -d '\n'
echo
