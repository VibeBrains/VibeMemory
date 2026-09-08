#!/usr/bin/env bash
# Finishes the migration once the last Claude session is gone.
#
# The last links cannot be re-aimed while a session is open — an open session appends to its
# transcript at any moment, and re-aiming the link under it would send those records to the old
# store. The owner therefore has to quit the application, and quitting it also ends the session
# that would have run the command. This watcher is the way out: it is started while the
# application is still running, waits for it to go, does the work, and writes down what happened.
#
# Safe to run twice: `switch` and the tick are idempotent, and the script stops on its own.
set -uo pipefail

engine="${VIBEMEMORY_DIR:-$HOME/.vibememory}"
binary="$engine/bin/vibememory"
log="$engine/finish-switch.log"
from="${1:-$HOME/OneDrive/.claude}"
# Long enough for a lunch break, short enough not to outlive the reason it was started.
deadline=$(( $(date +%s) + 12 * 60 * 60 ))
interval=30

say() { printf '%s %s\n' "$(date -u +%FT%TZ)" "$*" >> "$log"; }

# A session of this machine that is still running. The registry names the process; the process
# either answers or it does not, and a stale entry answers nothing.
live_sessions() {
  local count=0 file pid
  for file in "$HOME"/.claude/sessions/*.json; do
    [ -e "$file" ] || continue
    pid="$(basename "$file" .json)"
    case "$pid" in (*[!0-9]*) continue ;; esac
    kill -0 "$pid" 2>/dev/null && count=$(( count + 1 ))
  done
  printf '%s' "$count"
}

# The application itself: the card store cannot be switched while it is running, and it holds
# descriptors of its own.
desktop_running() {
  # Not `pgrep -x Claude`: it does not match the application (checked live — it finds nothing
  # while the app is plainly running). The full command line is what identifies it, and this is
  # the same method the engine itself uses.
  ps -axo comm | grep -q '^/Applications/Claude.app/Contents/MacOS/Claude$'
}

say "watcher started, waiting for the last session to end"

while :; do
  if [ "$(date +%s)" -ge "$deadline" ]; then
    say "gave up: still busy after 12 hours"
    exit 0
  fi
  sessions="$(live_sessions)"
  if [ "$sessions" = 0 ] && ! desktop_running; then
    break
  fi
  sleep "$interval"
done

say "all sessions ended; switching"
"$binary" switch --from "$from" --apply >> "$log" 2>&1
say "switch finished with status $?"

# The tick imports what is left — real directories become links, the store gets committed and
# pushed. It runs on a schedule anyway; running it here means the result is in this log.
"$binary" tick >> "$log" 2>&1
say "tick finished with status $?"

"$binary" doctor >> "$log" 2>&1
say "doctor finished with status $? — watcher done"
