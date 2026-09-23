#!/usr/bin/env bash
# Puts the memory server on the store's host, both ways a client without a local store reaches it:
# over ssh as a stdio server for the owner, and over HTTPS behind Caddy for every token of the
# access snapshot.
#
# Runs on the OWNER'S machine. The binary is built for x86_64 Linux beforehand (see
# docs/manuals/mcpServer.md) and passed with --binary; everything on the server happens in one ssh
# session with a here-document. The host must be prepared by hostBootstrap.sh first: the users vmgit
# and vmcab, their groups and /srv/vibememory.
#
# Idempotent: the binary is replaced by a rename and checked by running it. The first snapshot is
# made once, from the token file of the single-token server, and that file is removed only after
# the new server has been seen to accept the same token — clients keep working on the same value.
# The unit, the Caddyfile and the fail2ban jail are rewritten only when their text differs. Nothing
# of the store is touched, and no token is ever printed: this output is read in sessions whose
# transcripts are synced.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_DOMAIN=vibememory.ru
readonly DEFAULT_PORT=8787
readonly REPO_PATH=vibememory/store.git
# The owner's ssh command names this path; it becomes a link to the server's binary.
readonly OWNER_BIN=vibememory/bin/vibememory-mcp
readonly SERVER_BIN=/srv/vibememory/bin/vibememory-mcp
readonly ACCESS=/srv/vibememory/access/access.json
readonly TEAMS=/srv/vibememory/teams
# The token of the single-token server: made into the snapshot's legacy line, then removed.
readonly TOKEN_PATH=vibememory/mcp-token
# Limits of the personal store in the first snapshot: memories per project, bytes per memory.
readonly PERSONAL_MAX_RECORDS=5000
readonly PERSONAL_MAX_RECORD_BYTES=65536
# Softer than ssh (5): a client left with an old token after a rotation retries on its own, and
# locking the owner out of their own memory for an hour is the likelier ban than a real attacker.
readonly JAIL_MAXRETRY=10
readonly JAIL_FINDTIME=10m

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
domain="${VIBEMEMORY_DOMAIN:-$DEFAULT_DOMAIN}"
port="${VIBEMEMORY_MCP_PORT:-$DEFAULT_PORT}"
binary=""
owner=""

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Поставить сервер памяти на хост стора: по ssh и по HTTPS.

  ./infra/hostMcp.sh --binary <vibememory-mcp под x86_64 Linux> [--owner <handle владельца>]
                     [--alias vibememory] [--domain vibememory.ru] [--port 8787]

--owner нужен один раз, пока на хосте нет снимка прав: из файла токена прежнего сервера
собирается первый снимок, где владелец — единственный член личного стора.
Переменные окружения: VIBEMEMORY_SSH_ALIAS, VIBEMEMORY_DOMAIN, VIBEMEMORY_MCP_PORT.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --binary) binary="${2:-}"; shift 2 ;;
    --owner) owner="${2:-}"; shift 2 ;;
    --alias) sshAlias="${2:-}"; shift 2 ;;
    --domain) domain="${2:-}"; shift 2 ;;
    --port) port="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) fail "неизвестный аргумент $1" ;;
  esac
done

[ -n "$binary" ] || { usage; fail "нужен --binary"; }
[ -f "$binary" ] || fail "нет файла $binary"
case "$domain" in *[!a-zA-Z0-9.-]*|"") fail "домен $domain выглядит не как домен" ;; esac
case "$port" in *[!0-9]*|"") fail "порт $port — не число" ;; esac
# The same rule `access check` applies; checked here too, because the handle goes into JSON text.
case "$owner" in *[!a-z0-9-]*) fail "handle $owner: только строчные латинские буквы, цифры и дефис" ;; esac

say "1/6 Копирую бинарь"
ssh -o BatchMode=yes "$sshAlias" "mkdir -p \$HOME/$(dirname "$OWNER_BIN")"
scp -q "$binary" "$sshAlias:$OWNER_BIN.new"

ssh -o BatchMode=yes "$sshAlias" 'bash -s' -- "$domain" "$port" "$REPO_PATH" "$OWNER_BIN" \
  "$SERVER_BIN" "$ACCESS" "$TEAMS" "$TOKEN_PATH" "${owner:--}" "$PERSONAL_MAX_RECORDS" \
  "$PERSONAL_MAX_RECORD_BYTES" "$JAIL_MAXRETRY" "$JAIL_FINDTIME" <<'REMOTE'
set -euo pipefail
domain="$1"; port="$2"; repo="$HOME/$3"; ownerBin="$HOME/$4"; serverBin="$5"; access="$6"
teams="$7"; token="$HOME/$8"; owner="$9"; maxRecords="${10}"; maxRecordBytes="${11}"
jailMaxretry="${12}"; jailFindtime="${13}"
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
  echo "2/6 Снимок прав на месте и проходит проверку"
else
  [ -n "$owner" ] || { echo "Ошибка: снимка ещё нет — нужен --owner <handle владельца>" >&2; exit 1; }
  [ -s "$token" ] || { echo "Ошибка: нет ни снимка $access, ни файла токена $token" >&2; exit 1; }
  # The digest of the whole token as a client sends it: the file holds it without whitespace, and
  # the old server trimmed whatever it read.
  digest=$(tr -d '[:space:]' < "$token" | sha256sum | cut -d ' ' -f 1)
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
  "tokens": [
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
  ],
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
  echo "2/6 Снимок прав собран: личный стор, член $owner, строка tk_legacy с отпечатком токена"
fi

unit=/etc/systemd/system/vibememory-mcp.service
unitText="[Unit]
Description=VibeMemory memory server over HTTP (behind Caddy)
After=network.target

[Service]
User=vmgit
ExecStart=$serverBin --http 127.0.0.1:$port --access $access --teams $teams
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=$repo $teams
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
echo "3/6 Сервис vibememory-mcp под vmgit: $(systemctl is-active vibememory-mcp)"

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
  echo "4/6 Прежний токен принят новым сервером; его файл удалён — в снимке остался отпечаток"
else
  echo "4/6 Без токена — 401; файла прежнего токена нет"
fi

command -v caddy >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq caddy >/dev/null
caddyfile=/etc/caddy/Caddyfile
caddyText="$domain {
	handle /mcp {
		reverse_proxy 127.0.0.1:$port
	}
	handle {
		respond 404
	}
}"
if [ "$(sudo cat "$caddyfile" 2>/dev/null || true)" != "$caddyText" ]; then
  printf '%s\n' "$caddyText" | sudo tee "$caddyfile" >/dev/null
  sudo systemctl reload caddy 2>/dev/null || sudo systemctl restart caddy
fi
echo "5/6 Caddy: $(systemctl is-active caddy), https://$domain/mcp"

# The server logs every refused token with the address Caddy saw (the last X-Forwarded-For entry).
# Bans go to the web ports only: a wrong token says nothing about ssh. Without fail2ban on the host
# the step is skipped out loud — bootstrapping it is sshHardening.md's job, not this script's.
# Two things measured on the host, both of which leave a jail "active" that never counts a thing:
# the service runs as a user, so journald files its lines in a user journal, which the systemd
# backend skips unless journalflags=1 (LOCAL_ONLY); and the backend prefixes every message with
# host and process, so a failregex anchored at ^ matches nothing. An expired token is logged in
# other words, which the filter does not match: a client that was let in once is not an attack.
if ! command -v fail2ban-client >/dev/null && [ ! -x /usr/bin/fail2ban-client ]; then
  echo "6/6 fail2ban не установлен — джейл для /mcp пропущен"
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
  echo "6/6 fail2ban: джейл vibememory-mcp с действием $actions"
else
  echo "6/6 fail2ban: джейл vibememory-mcp БЕЗ действия — баны не дойдут до firewall" >&2
  exit 1
fi
REMOTE
