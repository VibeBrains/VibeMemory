#!/usr/bin/env bash
# Renders infra/Caddyfile.tmpl and puts it on the host: the one way the host's Caddyfile is written.
# hostMcp.sh and hostCabinet.sh both end with it, so neither can drop the other's site.
#
# Runs on the OWNER'S machine; the rendered text goes to the host through stdin. Idempotent: Caddy
# is reloaded only when the text differs, and a text Caddy refuses is never installed.
set -euo pipefail

readonly TEMPLATE="$(dirname "$0")/Caddyfile.tmpl"
readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_DOMAIN=vibememory.ru
readonly DEFAULT_APP_DOMAIN=app.vibememory.ru
readonly DEFAULT_MCP_PORT=8787
readonly DEFAULT_CABINET_PORT=3000
readonly RELEASES=/srv/vibememory/releases
readonly SITE=/srv/vibememory/site
# A lost SYN is retried instead of failing the run: the path to the host drops a connection now and
# then, and every step here is safe to repeat.
readonly SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=15 -o ConnectionAttempts=4)

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
domain="${VIBEMEMORY_DOMAIN:-$DEFAULT_DOMAIN}"
appDomain="${VIBEMEMORY_APP_DOMAIN:-$DEFAULT_APP_DOMAIN}"
mcpPort="${VIBEMEMORY_MCP_PORT:-$DEFAULT_MCP_PORT}"
cabinetPort="${VIBEMEMORY_CABINET_PORT:-$DEFAULT_CABINET_PORT}"

fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --alias) sshAlias="${2:-}"; shift 2 ;;
    --domain) domain="${2:-}"; shift 2 ;;
    --app-domain) appDomain="${2:-}"; shift 2 ;;
    --mcp-port) mcpPort="${2:-}"; shift 2 ;;
    --cabinet-port) cabinetPort="${2:-}"; shift 2 ;;
    -h | --help)
      printf 'Поставить Caddyfile хоста из infra/Caddyfile.tmpl.\n\n  ./infra/caddyApply.sh [--alias vibememory] [--domain %s] [--app-domain %s]\n' \
        "$DEFAULT_DOMAIN" "$DEFAULT_APP_DOMAIN"
      exit 0 ;;
    *) fail "неизвестный аргумент $1" ;;
  esac
done

for name in "$domain" "$appDomain"; do
  case "$name" in *[!a-zA-Z0-9.-]*|"") fail "домен $name выглядит не как домен" ;; esac
done
for port in "$mcpPort" "$cabinetPort"; do
  case "$port" in *[!0-9]*|"") fail "порт $port — не число" ;; esac
done
[ -f "$TEMPLATE" ] || fail "нет шаблона $TEMPLATE"

rendered=$(sed -e "s|{{DOMAIN}}|$domain|g" -e "s|{{APP_DOMAIN}}|$appDomain|g" \
  -e "s|{{MCP_PORT}}|$mcpPort|g" -e "s|{{CABINET_PORT}}|$cabinetPort|g" \
  -e "s|{{RELEASES}}|$RELEASES|g" -e "s|{{SITE}}|$SITE|g" "$TEMPLATE")
case "$rendered" in *'{{'*) fail "в шаблоне осталась неподставленная переменная" ;; esac

printf '%s\n' "$rendered" | ssh "${SSH_OPTIONS[@]}" "$sshAlias" "bash -c $(printf '%q' '
set -euo pipefail
candidate=$(mktemp)
trap "rm -f \"$candidate\"" EXIT
cat > "$candidate"
command -v caddy >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq caddy >/dev/null
# the releases directory exists before Caddy serves it, even if empty
sudo test -d /srv/vibememory/releases || sudo install -d -o root -g root -m 755 /srv/vibememory/releases
sudo test -d /srv/vibememory/site || sudo install -d -o root -g root -m 755 /srv/vibememory/site
caddyfile=/etc/caddy/Caddyfile
if sudo cmp -s "$candidate" "$caddyfile"; then
  echo "Caddy: без изменений ($(systemctl is-active caddy))"
  exit 0
fi
sudo caddy validate --adapter caddyfile --config "$candidate" >/dev/null 2>&1 ||
  { echo "Ошибка: Caddy не принимает новый Caddyfile, оставлен прежний" >&2; exit 1; }
sudo install -o root -g root -m 644 "$candidate" "$caddyfile"
sudo systemctl reload caddy 2>/dev/null || sudo systemctl restart caddy
echo "Caddy: Caddyfile обновлён ($(systemctl is-active caddy))"
')"
