#!/usr/bin/env bash
# Puts the memory server on the store's host, both ways a client without a local store reaches it:
# over ssh as a stdio server, and over HTTPS behind Caddy with a bearer token.
#
# Runs on the OWNER'S machine. The binary is built for x86_64 Linux beforehand (see
# docs/manuals/mcpServer.md) and passed with --binary; everything on the server happens in one ssh
# session with a here-document.
#
# Idempotent: the token is created once and never printed, the Caddyfile and the unit are
# rewritten only when their text differs, the binary is replaced by a rename and checked by
# running it. Nothing of the store is touched.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_DOMAIN=vibememory.ru
readonly DEFAULT_PORT=8787
readonly REPO_PATH=vibememory/store.git
readonly HOST_BIN=vibememory/bin/vibememory-mcp
readonly TOKEN_PATH=vibememory/mcp-token

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
say "1/4 Копирую бинарь"
ssh -o BatchMode=yes "$sshAlias" "mkdir -p \$HOME/$(dirname "$HOST_BIN")"
scp -q "$binary" "$sshAlias:$HOST_BIN.new"

ssh -o BatchMode=yes "$sshAlias" 'bash -s' -- "$domain" "$port" "$REPO_PATH" "$HOST_BIN" "$TOKEN_PATH" <<'REMOTE'
set -euo pipefail
domain="$1"; port="$2"; repo="$HOME/$3"; bin="$HOME/$4"; token="$HOME/$5"

chmod 755 "$bin.new"
"$bin.new" --version >/dev/null || { echo "Ошибка: новый бинарь не запускается" >&2; exit 1; }
mv -f "$bin.new" "$bin"
echo "2/4 Бинарь на месте: $("$bin" --version)"

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
echo "3/4 Сервис vibememory-mcp: $(systemctl is-active vibememory-mcp)"

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
echo "4/4 Caddy: $(systemctl is-active caddy), https://$domain/mcp"
REMOTE
