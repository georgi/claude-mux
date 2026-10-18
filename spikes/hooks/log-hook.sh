#!/bin/bash
# usage: log-hook.sh <tag>; logs stdin JSON to $SPIKE_LOG (default hooks/log.jsonl) tagged; prints $SPIKE_RESP_<tag> file contents if exists
D="$(cd "$(dirname "$0")" && pwd)"
tag="$1"
body="$(cat)"
printf '{"tag":"%s","ts":"%s","payload":%s}\n' "$tag" "$(date +%T.%N)" "$body" >> "${SPIKE_LOG:-$D/log.jsonl}"
[ -f "$D/sleep-$tag" ] && sleep "$(cat "$D/sleep-$tag")"
f="$D/resp-$tag.json"
[ -f "$f" ] && cat "$f"
exit 0
