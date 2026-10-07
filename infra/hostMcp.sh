#!/usr/bin/env bash
# Puts the memory server on the store's host, both ways a client without a local store reaches it:
# over ssh as a stdio server for the owner, and over HTTPS behind Caddy for every token of the
# access snapshot. With it come the host's own commands under vmgit: the application of the access
# snapshot whenever it changes, the host's report every hour, and the nightly repacking of the team
# stores.
#
# Runs on the OWNER'S machine. The binary is built for x86_64 Linux beforehand (see
# docs/manuals/mcpServer.md) and passed with --binary; everything on the server happens in one ssh
# session with a here-document. The host must be prepared by hostBootstrap.sh first: the users vmgit
# and vmcab, their groups and /srv/vibememory.
#
# Idempotent: the binary is replaced by a rename and checked by running it. The first snapshot is
# made once, from the token file of the single-token server, and that file is removed only after
# the new server has been seen to accept the same token — clients keep working on the same value.
# The units and the fail2ban jail are rewritten only when their text differs, and the Caddyfile comes
# from infra/Caddyfile.tmpl through caddyApply.sh, the one writer of that file. Nothing
# of the store is touched, and no token is ever printed: this output is read in sessions whose
# transcripts are synced.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_DOMAIN=vibememory.ru
# The cabinet: a refusal only the cabinet can lift (a team whose term ran out) names its address.
readonly DEFAULT_APP_DOMAIN=app.vibememory.ru
readonly DEFAULT_PORT=8787
readonly REPO_PATH=vibememory/store.git
# The owner's ssh command names this path; it becomes a link to the server's binary.
readonly OWNER_BIN=vibememory/bin/vibememory-mcp
readonly SERVER_BIN=/srv/vibememory/bin/vibememory-mcp
readonly ACCESS=/srv/vibememory/access/access.json
readonly TEAMS=/srv/vibememory/teams
# Installed by hostBootstrap.sh; repacks every live team store.
readonly STORE_REPACK=/srv/vibememory/bin/storeRepack.sh
# vmgit's keys: written by the application of the snapshot alone.
readonly VMGIT_SSH=/home/vmgit/.ssh
# After the nightly backup (backupSetup.sh, 03:47) and the owner's repack (hostBootstrap.sh, 04:17).
readonly TEAMS_REPACK_AT='*-*-* 04:47:00'
# How often a failed application of the snapshot is tried again, in minutes.
readonly ACCESS_CATCH_UP_MINUTES=15
# The token of the single-token server: made into the snapshot's legacy line, then removed.
readonly TOKEN_PATH=vibememory/mcp-token
# Limits of the personal store in the first snapshot: memories per project, bytes per memory.
readonly PERSONAL_MAX_RECORDS=5000
readonly PERSONAL_MAX_RECORD_BYTES=65536
# Softer than ssh (5): a client left with an old token after a rotation retries on its own, and
# locking the owner out of their own memory for an hour is the likelier ban than a real attacker.
readonly JAIL_MAXRETRY=10
readonly JAIL_FINDTIME=10m
# A lost SYN is retried instead of failing the run: the path to the host drops a connection now and
# then, and every step here is safe to repeat.
readonly SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=15 -o ConnectionAttempts=4)

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
domain="${VIBEMEMORY_DOMAIN:-$DEFAULT_DOMAIN}"
appDomain="${VIBEMEMORY_APP_DOMAIN:-$DEFAULT_APP_DOMAIN}"
port="${VIBEMEMORY_MCP_PORT:-$DEFAULT_PORT}"
binary=""
owner=""

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Поставить сервер памяти на хост стора: по ssh и по HTTPS.

  ./infra/hostMcp.sh --binary <vibememory-mcp под x86_64 Linux> [--owner <handle владельца>]
                     [--alias vibememory] [--domain vibememory.ru] [--app-domain app.vibememory.ru]
                     [--port 8787]

--owner нужен один раз, пока на хосте нет снимка прав: из файла токена прежнего сервера
собирается первый снимок, где владелец — единственный член личного стора.
Переменные окружения: VIBEMEMORY_SSH_ALIAS, VIBEMEMORY_DOMAIN, VIBEMEMORY_APP_DOMAIN, VIBEMEMORY_MCP_PORT.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --binary) binary="${2:-}"; shift 2 ;;
    --owner) owner="${2:-}"; shift 2 ;;
    --alias) sshAlias="${2:-}"; shift 2 ;;
    --domain) domain="${2:-}"; shift 2 ;;
    --app-domain) appDomain="${2:-}"; shift 2 ;;
    --port) port="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) fail "неизвестный аргумент $1" ;;
  esac
done

[ -n "$binary" ] || { usage; fail "нужен --binary"; }
[ -f "$binary" ] || fail "нет файла $binary"
for name in "$domain" "$appDomain"; do
  case "$name" in *[!a-zA-Z0-9.-]*|"") fail "домен $name выглядит не как домен" ;; esac
done
case "$port" in *[!0-9]*|"") fail "порт $port — не число" ;; esac
# The same rule `access check` applies; checked here too, because the handle goes into JSON text.
case "$owner" in *[!a-z0-9-]*) fail "handle $owner: только строчные латинские буквы, цифры и дефис" ;; esac

say "1/7 Копирую бинарь"
ssh -n "${SSH_OPTIONS[@]}" "$sshAlias" "mkdir -p \$HOME/$(dirname "$OWNER_BIN")"
scp -q -o ConnectTimeout=15 -o ConnectionAttempts=4 "$binary" "$sshAlias:$OWNER_BIN.new"

ssh "${SSH_OPTIONS[@]}" "$sshAlias" "bash -s -- $(printf '%q ' "$domain" "$port" "$REPO_PATH" \
  "$OWNER_BIN" "$SERVER_BIN" "$ACCESS" "$TEAMS" "$TOKEN_PATH" "${owner:--}" "$PERSONAL_MAX_RECORDS" \
  "$PERSONAL_MAX_RECORD_BYTES" "$JAIL_MAXRETRY" "$JAIL_FINDTIME" "$STORE_REPACK" "$VMGIT_SSH" \
  "$TEAMS_REPACK_AT" "$appDomain" "$ACCESS_CATCH_UP_MINUTES")" <<'REMOTE'
set -euo pipefail
domain="$1"; port="$2"; repo="$HOME/$3"; ownerBin="$HOME/$4"; serverBin="$5"; access="$6"
teams="$7"; token="$HOME/$8"; owner="$9"; maxRecords="${10}"; maxRecordBytes="${11}"
jailMaxretry="${12}"; jailFindtime="${13}"; storeRepack="${14}"; vmgitSsh="${15}"
teamsRepackAt="${16}"; appDomain="${17}"; catchUpMinutes="${18}"
# ssh glues arguments into one line and an empty one vanishes; "-" stands for "not given".
[ "$owner" = - ] && owner=""

for account in vmgit vmcab; do
  id "$account" >/dev/null 2>&1 || { echo "Ошибка: нет пользователя $account — сначала hostBootstrap.sh" >&2; exit 1; }
done

# The binary: owned by root, so the server that runs it cannot change it; replaced by a rename, so
# a running server keeps its file and systemd never starts a half-copied one.
chmod 755 "$ownerBin.new"
"$ownerBin.new" --version >/dev/null || { echo "Ошибка: новый бинарь не запускается" >&2; exit 1; }
sudo install -o root -g root -m 755 "$ownerBin.new" "$serverBin.new"
sudo mv -f "$serverBin.new" "$serverBin"
rm -f "$ownerBin.new"
if [ "$(readlink "$ownerBin" 2>/dev/null || true)" != "$serverBin" ]; then
  ln -sfn "$serverBin" "$ownerBin.link"
  mv -Tf "$ownerBin.link" "$ownerBin"
fi
echo "   бинарь: $("$serverBin" --version), команда владельца по ssh — ссылка на него"

# The snapshot: checked as the server's own user, which also proves that user can read it.
check() { sudo -u vmgit "$serverBin" access check "$1" | head -n 1 || true; }
if sudo test -e "$access"; then
  verdict=$(check "$access")
  [ "$verdict" = ok ] || { echo "Ошибка: снимок $access не проходит проверку: $verdict" >&2; exit 1; }
  echo "2/7 Снимок прав на месте и проходит проверку"
else
  [ -n "$owner" ] || { echo "Ошибка: снимка ещё нет — нужен --owner <handle владельца>" >&2; exit 1; }
  # A host moving from the single-token server keeps that token: its digest becomes the snapshot's legacy line
  # A new host has no token yet: its agents get theirs from the console
  tokens='[]'
  made="без токенов — их выдаёт консоль"
  if [ -s "$token" ]; then
    # The digest of the whole token as a client sends it: the file holds it without whitespace, and
    # The old server trimmed whatever it read
    digest=$(tr -d '[:space:]' < "$token" | sha256sum | cut -d ' ' -f 1)
    tokens=$(cat <<JSON
[
    {
      "id": "tk_legacy",
      "legacy": true,
      "sha256": "$digest",
      "team": "personal",
      "member": "$owner",
      "agent": "claude-code",
      "role": "writer",
      "history": true,
      "projects": null,
      "expiresAt": null
    }
  ]
JSON
)
    made="строка tk_legacy с отпечатком токена"
  fi
  candidate=$(mktemp)
  cat > "$candidate" <<JSON
{
  "version": 1,
  "teamCount": 1,
  "teams": {
    "personal": {
      "adopted": true,
      "repo": "$repo",
      "writable": true,
      "limits": { "maxRecords": $maxRecords, "maxRecordBytes": $maxRecordBytes },
      "members": { "$owner": "owner" }
    }
  },
  "tokens": $tokens,
  "keys": []
}
JSON
  chmod 644 "$candidate"
  verdict=$(check "$candidate")
  [ "$verdict" = ok ] || { rm -f "$candidate"; echo "Ошибка: собранный снимок не проходит проверку: $verdict" >&2; exit 1; }
  # Owned by the cabinet's user, as if the cabinet had written it: in the sticky directory only
  # the file's owner may later rename a new snapshot over it.
  sudo install -o vmcab -g vmaccess -m 0640 "$candidate" "$access.new"
  sudo mv -f "$access.new" "$access"
  rm -f "$candidate"
  echo "2/7 Снимок прав собран: личный стор, член $owner, $made"
fi

# When each team was last reached: the server touches a file a team, the report reads its time.
# What each token asked for outside its list of projects: a file a token, read into the report.
# The cabinet's archive requests and the host's archives: the same shared group as the snapshot,
# sticky, so each side removes only its own files.
activity=$(dirname "$teams")/activity
exports=$(dirname "$teams")/exports
outside=$(dirname "$teams")/outside
sudo install -d -o vmgit -g vmgit -m 0750 "$activity"
sudo install -d -o vmgit -g vmgit -m 0750 "$outside"
sudo install -d -o root -g vmaccess -m 3770 "$exports"

unit=/etc/systemd/system/vibememory-mcp.service
unitText="[Unit]
Description=VibeMemory memory server over HTTP (behind Caddy)
After=network.target

[Service]
User=vmgit
ExecStart=$serverBin --http 127.0.0.1:$port --access $access --teams $teams --cabinet https://$appDomain
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=$repo $teams $activity $outside
ReadOnlyPaths=$(dirname "$access")

[Install]
WantedBy=multi-user.target"
if [ "$(sudo cat "$unit" 2>/dev/null || true)" != "$unitText" ]; then
  printf '%s\n' "$unitText" | sudo tee "$unit" >/dev/null
  sudo systemctl daemon-reload
fi
sudo systemctl enable --quiet vibememory-mcp
sudo systemctl restart vibememory-mcp
for _ in $(seq 1 20); do
  curl -s -o /dev/null "http://127.0.0.1:$port/mcp" && break
  sleep 0.5
done
echo "3/7 Сервис vibememory-mcp под vmgit: $(systemctl is-active vibememory-mcp)"

status() { curl -s -o /dev/null -w '%{http_code}' "$@" "http://127.0.0.1:$port/mcp"; }
anonymous=$(status -X POST -H 'Content-Type: application/json' --data '{}')
[ "$anonymous" = 401 ] || { echo "Ошибка: без токена сервер ответил $anonymous, а не 401" >&2; exit 1; }
if [ -s "$token" ]; then
  # The token goes to curl in a file, not in its arguments: those are visible in the process list.
  header=$(mktemp)
  chmod 600 "$header"
  printf 'Authorization: Bearer %s\n' "$(tr -d '[:space:]' < "$token")" > "$header"
  pinged=$(status -H @"$header" -H 'Content-Type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"ping"}')
  rm -f "$header"
  [ "$pinged" = 200 ] || { echo "Ошибка: новый сервер не принял прежний токен (код $pinged); файл токена оставлен" >&2; exit 1; }
  rm -f "$token"
  echo "4/7 Прежний токен принят новым сервером; его файл удалён — в снимке остался отпечаток"
else
  echo "4/7 Без токена — 401; файла прежнего токена нет"
fi

# The host's own commands, all under vmgit, each unit writing exactly where its command writes:
# the application of the snapshot (team stores, vmgit's keys, applied.json, host.json) whenever
# the cabinet publishes one, the report every hour, and the nightly repacking of the team stores.
[ -x "$storeRepack" ] || { echo "Ошибка: нет $storeRepack — сначала hostBootstrap.sh" >&2; exit 1; }
# The host's locks live where only vmgit writes: the exchange directory is the cabinet's to write
# too, and a lock it could hold or swap for a link would stall or misdirect the host's commands.
locks="$(dirname "$teams")/locks"
sudo install -d -o vmgit -g vmgit -m 0700 "$locks"
sudo rm -f "$(dirname "$access")/.access-apply.lock" "$(dirname "$access")/.host.json.lock"
reload=0
putUnit() {
  local file="/etc/systemd/system/$1" text="$2"
  if [ "$(sudo cat "$file" 2>/dev/null || true)" != "$text" ]; then
    printf '%s\n' "$text" | sudo tee "$file" >/dev/null
    reload=1
  fi
}
applyService() {
  printf '%s\n' "[Unit]
Description=VibeMemory: $1

[Service]
Type=oneshot
User=vmgit
UMask=0007
ExecStart=$serverBin access-apply --access $access --teams $teams$2
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=$teams $(dirname "$access") $locks $vmgitSsh"
}
putUnit vibememory-access-apply.service "$(applyService "apply the access snapshot to the team stores and vmgit's keys" "")"
# An application that failed is otherwise retried only by the next publication, which may be days
# away. The catch-up run does nothing while the last application of the file did everything, and
# waits for one under way: the binary holds a lock over the whole application.
putUnit vibememory-access-catch-up.service "$(applyService "apply the access snapshot again if its last application failed" " --catch-up")"
putUnit vibememory-access-catch-up.timer "[Unit]
Description=VibeMemory: catch up with the access snapshot every $catchUpMinutes minutes

[Timer]
OnCalendar=*:0/$catchUpMinutes
Persistent=true

[Install]
WantedBy=timers.target"
putUnit vibememory-access-apply.path "[Unit]
Description=VibeMemory: apply the access snapshot whenever it changes

[Path]
PathChanged=$access
Unit=vibememory-access-apply.service

[Install]
WantedBy=multi-user.target"
putUnit vibememory-status.service "[Unit]
Description=VibeMemory: the host's report for the cabinet

[Service]
Type=oneshot
User=vmgit
UMask=0007
ExecStart=$serverBin status --access $access --teams $teams
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=$(dirname "$access") $locks"
putUnit vibememory-status.timer "[Unit]
Description=VibeMemory: the host's report every hour

[Timer]
OnCalendar=hourly
Persistent=true

[Install]
WantedBy=timers.target"
putUnit vibememory-export.service "[Unit]
Description=VibeMemory: build the memory archives the cabinet asked for

[Service]
Type=oneshot
User=vmgit
UMask=0007
ExecStart=$serverBin export --access $access --teams $teams
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=$exports"
putUnit vibememory-export.path "[Unit]
Description=VibeMemory: build an archive whenever the cabinet asks for one

[Path]
PathChanged=$exports
Unit=vibememory-export.service

[Install]
WantedBy=multi-user.target"
# the path unit answers a request at once; the timer removes old archives and catches a missed one
putUnit vibememory-export.timer "[Unit]
Description=VibeMemory: archives every hour

[Timer]
OnCalendar=hourly
Persistent=true

[Install]
WantedBy=timers.target"
putUnit vibememory-teams-repack.service "[Unit]
Description=VibeMemory: nightly repacking of the team stores

[Service]
Type=oneshot
User=vmgit
UMask=0007
ExecStart=$storeRepack --teams $teams
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=$teams"
putUnit vibememory-teams-repack.timer "[Unit]
Description=VibeMemory: repack the team stores every night

[Timer]
OnCalendar=$teamsRepackAt
Persistent=true

[Install]
WantedBy=timers.target"
[ "$reload" = 0 ] || sudo systemctl daemon-reload
sudo systemctl enable --now --quiet vibememory-access-apply.path vibememory-access-catch-up.timer \
  vibememory-status.timer vibememory-teams-repack.timer vibememory-export.path vibememory-export.timer
# Applied once now rather than at the cabinet's first publication: vmgit's keys and the host's
# report exist from the start.
sudo systemctl start vibememory-access-apply.service ||
  { echo "Ошибка: применение снимка не прошло — journalctl -t vibememory-apply" >&2; exit 1; }
echo "5/7 Команды хоста под vmgit: снимок применён ($(sudo -u vmgit cat "$(dirname "$access")/applied.json" | grep -c '"code"') проблем), догон — раз в $catchUpMinutes мин, отчёт — раз в час, упаковка сторов команд — $teamsRepackAt"

# The server logs every refused token with the address Caddy saw (the last X-Forwarded-For entry).
# Bans go to the web ports only: a wrong token says nothing about ssh. Without fail2ban on the host
# the step is skipped out loud — bootstrapping it is sshHardening.md's job, not this script's.
# Two things measured on the host, both of which leave a jail "active" that never counts a thing:
# the service runs as a user, so journald files its lines in a user journal, which the systemd
# backend skips unless journalflags=1 (LOCAL_ONLY); and the backend prefixes every message with
# host and process, so a failregex anchored at ^ matches nothing. An expired token is logged in
# other words, which the filter does not match: a client that was let in once is not an attack.
# The address in the line is an IP literal (http.rs::client_address), and usedns = no besides: a
# name is never resolved into somebody's address to ban.
if ! command -v fail2ban-client >/dev/null && [ ! -x /usr/bin/fail2ban-client ]; then
  echo "6/7 fail2ban не установлен — джейл для /mcp пропущен: поставьте его по шагу 3 docs/manuals/serverHardeningPrompt.md и повторите"
  exit 0
fi
filter=/etc/fail2ban/filter.d/vibememory-mcp.conf
filterText="[Definition]
failregex = vibememory-mcp: refused a request without the right token from <HOST>\$
journalmatch = _SYSTEMD_UNIT=vibememory-mcp.service"
jail=/etc/fail2ban/jail.d/vibememory-mcp.local
jailText="[vibememory-mcp]
enabled = true
filter = vibememory-mcp
backend = systemd[journalflags=1]
usedns = no
port = http,https
maxretry = $jailMaxretry
findtime = $jailFindtime
banaction = nftables-multiport"
changed=0
if [ "$(sudo cat "$filter" 2>/dev/null || true)" != "$filterText" ]; then
  printf '%s\n' "$filterText" | sudo tee "$filter" >/dev/null; changed=1
fi
if [ "$(sudo cat "$jail" 2>/dev/null || true)" != "$jailText" ]; then
  printf '%s\n' "$jailText" | sudo tee "$jail" >/dev/null; changed=1
fi
# Restart, not reload: on fail2ban 1.1 a reload that changes the action of an existing jail leaves
# that jail running with no action at all — it keeps counting and "banning" into its own database
# while nothing reaches the firewall. A restart rebuilds the actions and restores bans from the
# database.
if [ "$changed" = 1 ]; then
  sudo systemctl restart fail2ban
  for _ in $(seq 1 20); do sudo /usr/bin/fail2ban-client ping >/dev/null 2>&1 && break; sleep 1; done
fi
# "active" is not the claim that matters; an action that can reach the firewall is.
actions=$(sudo /usr/bin/fail2ban-client get vibememory-mcp actions 2>/dev/null | tail -n +2)
if [ -n "$actions" ]; then
  echo "6/7 fail2ban: джейл vibememory-mcp с действием $actions"
else
  echo "6/7 fail2ban: джейл vibememory-mcp БЕЗ действия — баны не дойдут до firewall" >&2
  exit 1
fi
REMOTE

# Caddy last, from the template both host scripts share: the cabinet's site stays in the same file.
say "7/7 Caddy"
"$(dirname "$0")/caddyApply.sh" --alias "$sshAlias" --domain "$domain" --app-domain "$appDomain" --mcp-port "$port"
