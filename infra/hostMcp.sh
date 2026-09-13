#!/usr/bin/env bash
# Puts the memory server on the store's host, both ways a client without a local store reaches it:
# over ssh as a stdio server, and over HTTPS behind Caddy with a bearer token.
#
# Runs on the OWNER'S machine. The binary is built for x86_64 Linux beforehand (see
# docs/manuals/mcpServer.md) and passed with --binary; everything on the server happens in one ssh
# session with a here-document.
#
# Idempotent: the token is created once and never printed, the Caddyfile, the unit and the
# fail2ban jail are rewritten only when their text differs, the binary is replaced by a rename and
# checked by running it. Nothing of the store is touched.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_DOMAIN=vibememory.ru
readonly DEFAULT_PORT=8787
readonly REPO_PATH=vibememory/store.git
readonly HOST_BIN=vibememory/bin/vibememory-mcp
readonly TOKEN_PATH=vibememory/mcp-token
# Softer than ssh (5): a client left with an old token after a rotation retries on its own, and
# locking the owner out of their own memory for an hour is the likelier ban than a real attacker.
readonly JAIL_MAXRETRY=10
readonly JAIL_FINDTIME=10m

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
domain="${VIBEMEMORY_DOMAIN:-$DEFAULT_DOMAIN}"
port="${VIBEMEMORY_MCP_PORT:-$DEFAULT_PORT}"
binary=""

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Поставить сервер памяти на хост стора: по ssh и по HTTPS.

  ./infra/hostMcp.sh --binary <vibememory-mcp под x86_64 Linux> [--alias vibememory]
                     [--domain vibememory.ru] [--port 8787]

Переменные окружения: VIBEMEMORY_SSH_ALIAS, VIBEMEMORY_DOMAIN, VIBEMEMORY_MCP_PORT.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --binary) binary="${2:-}"; shift 2 ;;
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

# The binary goes beside its final name and is renamed there: a running server keeps the old file,
# and a half-copied binary is never the one systemd starts.
say "1/5 Копирую бинарь"
ssh -o BatchMode=yes "$sshAlias" "mkdir -p \$HOME/$(dirname "$HOST_BIN")"
scp -q "$binary" "$sshAlias:$HOST_BIN.new"

ssh -o BatchMode=yes "$sshAlias" 'bash -s' -- "$domain" "$port" "$REPO_PATH" "$HOST_BIN" "$TOKEN_PATH" \
  "$JAIL_MAXRETRY" "$JAIL_FINDTIME" <<'REMOTE'
set -euo pipefail
domain="$1"; port="$2"; repo="$HOME/$3"; bin="$HOME/$4"; token="$HOME/$5"
jailMaxretry="$6"; jailFindtime="$7"

chmod 755 "$bin.new"
"$bin.new" --version >/dev/null || { echo "Ошибка: новый бинарь не запускается" >&2; exit 1; }
mv -f "$bin.new" "$bin"
echo "2/5 Бинарь на месте: $("$bin" --version)"

# 256 bits from the kernel, hex. Created once; never printed — this output is read in a session
# whose transcript is synced, and a token in a transcript is a token in a git history.
if [ ! -s "$token" ]; then
  umask 077
  head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$token"
fi
chmod 600 "$token"

unit=/etc/systemd/system/vibememory-mcp.service
unitText="[Unit]
Description=VibeMemory memory server over HTTP (behind Caddy)
After=network.target

[Service]
User=$(id -un)
ExecStart=$bin --git-store $repo --machine host --http 127.0.0.1:$port --token-file $token
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=$repo

[Install]
WantedBy=multi-user.target"
if [ "$(sudo cat "$unit" 2>/dev/null || true)" != "$unitText" ]; then
  printf '%s\n' "$unitText" | sudo tee "$unit" >/dev/null
  sudo systemctl daemon-reload
fi
sudo systemctl enable --quiet vibememory-mcp
sudo systemctl restart vibememory-mcp
echo "3/5 Сервис vibememory-mcp: $(systemctl is-active vibememory-mcp)"

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
echo "4/5 Caddy: $(systemctl is-active caddy), https://$domain/mcp"

# The server logs every refused token with the address Caddy saw (the last X-Forwarded-For entry).
# Bans go to the web ports only: a wrong token says nothing about ssh. Without fail2ban on the host
# the step is skipped out loud — bootstrapping it is sshHardening.md's job, not this script's.
# Two things measured on the host, both of which leave a jail "active" that never counts a thing:
# the service runs as a user, so journald files its lines in user-1000.journal, which the systemd
# backend skips unless journalflags=1 (LOCAL_ONLY); and the backend prefixes every message with
# host and process, so a failregex anchored at ^ matches nothing.
if ! command -v fail2ban-client >/dev/null && [ ! -x /usr/bin/fail2ban-client ]; then
  echo "5/5 fail2ban не установлен — джейл для /mcp пропущен"
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
[ "$changed" = 0 ] || sudo /usr/bin/fail2ban-client reload >/dev/null
echo "5/5 fail2ban: джейл vibememory-mcp $(sudo /usr/bin/fail2ban-client status vibememory-mcp >/dev/null 2>&1 && echo active || echo 'НЕ поднялся')"
REMOTE
