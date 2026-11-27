# shellcheck shell=sh
# The end of a gateway recording, sourced by hermes/record.sh and openclaw/record.sh: keep the gateway's log
# when RECORD_LOG asks for it, then publish the recording only from a run that succeeded and reached the end
# of its scenario. Otherwise it fails and leaves the destination as it was.

# publish_recording <container-status> <work-dir> <key-file> <output.jsonl>
# <work-dir> is the recorder's owner-only folder: gateway.log, and out/recording.jsonl as the container wrote it.
publish_recording() {
    if [ -n "${RECORD_LOG:-}" ]; then
        # Never keep a line that holds the key, whatever put it there. The log is made in the work folder
        # and moved into place, so it is owner-only even where RECORD_LOG already exists.
        grep -v -F -f "$3" "$2/gateway.log" > "$2/log" || true
        mv -f "$2/log" "$RECORD_LOG"
    fi
    if [ "$1" -ne 0 ]; then
        echo "error: the container exited with status $1; nothing was written" >&2
        return 1
    fi
    # The recorder plugin ends every scenario it gets through with a "finished" note, whatever its outcome.
    if ! python3 -c '
import json, sys
try:
    records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8").read().split("\n") if line.strip()]
except (OSError, ValueError):
    sys.exit(1)
finished = any(isinstance(r, dict) and r.get("via") == "note" and isinstance(r.get("msg"), dict)
               and "finished" in r["msg"] for r in records)
sys.exit(0 if finished else 1)
' "$2/out/recording.jsonl"; then
        echo "error: the recording is missing or did not reach the end of the scenario; nothing was written" >&2
        return 1
    fi
    mkdir -p "$(dirname "$4")"
    # Copied beside the destination and renamed over it, so it never holds part of a recording.
    tmp=$(mktemp "$4.XXXXXX")
    cp "$2/out/recording.jsonl" "$tmp" || { rm -f "$tmp"; return 1; }
    mv -f "$tmp" "$4"
    echo "wrote $4"
}
