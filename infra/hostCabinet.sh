#!/usr/bin/env bash
# Puts the cabinet on the store's host: Postgres 17 from PGDG, Bun under vmcab, the built dist/, the
# database's migrations, the owner seeded from the host's own access snapshot, and the service
# behind Caddy with its fail2ban jail.
#
# Runs on the OWNER'S machine after hostBootstrap.sh and hostMcp.sh. The cabinet is built here — the
# host neither builds nor type-checks (a build there was killed for memory on the VibeSter host) — and
# only what runs it travels: dist/, the manifests, the Prisma schema and its migrations. Everything
# on the server happens in one ssh session with a here-document.
#
# Order on an empty database, which is the point of the order: migrations, then the import of the
# owner from /srv/vibememory/access/access.json, then the service. The service's ExecStartPre asks
# `access:seeded` and refuses to start on a database without its owner — so a cabinet can never
# publish an empty snapshot over the host's real one. Without --owner-email on the first run the
# import waits, and the service does not start: that is not a failure of this script.
#
# Idempotent: packages, roles, the database, Bun, the unit, the jail and the journal limit change
# only where they differ; the release is replaced and the service restarted only when the built
# release or the .env differs. Secrets are generated on the host once and kept there (.env, 0600,
# vmcab); nothing secret is printed, and nothing secret is ever passed as an argument.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_DOMAIN=vibememory.ru
readonly DEFAULT_APP_DOMAIN=app.vibememory.ru
readonly CABINET_PORT=3000
readonly MCP_PORT=8787
readonly CABINET_LOCAL="$(cd "$(dirname "$0")/../cabinet" && pwd)"
readonly RELEASES_LOCAL=/Volumes/Storage/Caches/VibeMemory/cabinetReleases
# The Bun the cabinet is built and tested with (package.json devDependencies).
readonly BUN_VERSION=1.4.0
readonly PG_VERSION=17
# Postgres on a 4 GB host shared with the stores: small buffers, and a connection cap the cabinet's
# pools stay under (DATABASE_POOL_MAX + PGBOSS_POOL_MAX + BACKPLANE_POOL_MAX + LISTEN, plus room for a
# migration and the nightly dump).
readonly PG_SHARED_BUFFERS=64MB
readonly PG_MAX_CONNECTIONS=20
readonly JOURNAL_MAX_USE=200M
# As the memory server's jail: a person mistyping a password is the likelier ban than an attacker.
readonly JAIL_MAXRETRY=10
readonly JAIL_FINDTIME=10m

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
domain="${VIBEMEMORY_DOMAIN:-$DEFAULT_DOMAIN}"
appDomain="${VIBEMEMORY_APP_DOMAIN:-$DEFAULT_APP_DOMAIN}"
ownerEmail=""
skipBuild=0

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Выложить кабинет на хост: Postgres, Bun, dist/, миграции, импорт владельца, служба за Caddy.

  ./infra/hostCabinet.sh [--owner-email <адрес владельца>] [--skip-build]
                         [--alias vibememory] [--domain $DEFAULT_DOMAIN] [--app-domain $DEFAULT_APP_DOMAIN]

  --owner-email  адрес учётки владельца в кабинете; нужен один раз, для импорта владельца из
                 снимка прав хоста. Без него импорт ждёт, и служба не стартует — так задумано.
  --skip-build   не собирать кабинет заново, выложить последнюю сборку из $RELEASES_LOCAL.

Пароль владельца скрипт не ставит: его вводит сам владелец, ./infra/cabinetPassword.sh.
Ключ Resend и бот Telegram вносит тоже владелец: ./infra/cabinetSecrets.sh.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --owner-email) ownerEmail="${2:-}"; shift 2 ;;
    --skip-build) skipBuild=1; shift ;;
    --alias) sshAlias="${2:-}"; shift 2 ;;
    --domain) domain="${2:-}"; shift 2 ;;
    --app-domain) appDomain="${2:-}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) fail "неизвестный аргумент $1 (--help покажет список)" ;;
  esac
done

for name in "$domain" "$appDomain"; do
  case "$name" in *[!a-zA-Z0-9.-]*|"") fail "домен $name выглядит не как домен" ;; esac
done
case "$ownerEmail" in
  "") ;;
  *[!a-zA-Z0-9.@_+-]*|*@*@*|@*|*@) fail "адрес $ownerEmail выглядит не как адрес" ;;
  *@*) ;;
  *) fail "адрес $ownerEmail выглядит не как адрес" ;;
esac
command -v bun >/dev/null || fail "нет bun на этой машине: кабинет собирается здесь"
ssh -o BatchMode=yes "$sshAlias" 'echo ok' >/dev/null 2>&1 || fail "сервер не пускает по ключу"

# 1. The release: built here, packed with exactly what runs it.
sourceVersion=$(git -C "$CABINET_LOCAL" rev-parse --short=12 HEAD)
[ -z "$(git -C "$CABINET_LOCAL" status --porcelain -- .)" ] ||
  say "   внимание: в cabinet/ есть незакоммиченные правки — SOURCE_VERSION их не описывает"
mkdir -p "$RELEASES_LOCAL"
release="$RELEASES_LOCAL/cabinet-$sourceVersion.tar.gz"
if [ "$skipBuild" = 0 ]; then
  say "1/9 Собираю кабинет ($sourceVersion)"
  (cd "$CABINET_LOCAL" && bun run build:code >/dev/null)
  tar -czf "$release" -C "$CABINET_LOCAL" dist package.json bun.lock prisma.config.ts \
    src/modules/prisma/schema.prisma src/modules/prisma/migrations
else
  [ -f "$release" ] || fail "нет сборки $release — запустите без --skip-build"
  say "1/9 Сборка $sourceVersion взята готовой"
fi
releaseHash=$(shasum -a 256 "$release" | cut -d ' ' -f 1)
scp -q "$release" "$sshAlias:cabinet-release.tar.gz"

ssh -o BatchMode=yes "$sshAlias" "bash -s -- $(printf '%q ' "$domain" "$appDomain" "$sourceVersion" \
  "$releaseHash" "${ownerEmail:--}" "$BUN_VERSION" "$PG_VERSION" "$PG_SHARED_BUFFERS" \
  "$PG_MAX_CONNECTIONS" "$JOURNAL_MAX_USE" "$JAIL_MAXRETRY" "$JAIL_FINDTIME" "$CABINET_PORT" \
  "$MCP_PORT")" <<'REMOTE'
set -euo pipefail
domain="$1"; appDomain="$2"; sourceVersion="$3"; releaseHash="$4"; ownerEmail="$5"
bunVersion="$6"; pgVersion="$7"; sharedBuffers="$8"; maxConnections="$9"; journalMaxUse="${10}"
jailMaxretry="${11}"; jailFindtime="${12}"; cabinetPort="${13}"; mcpPort="${14}"
[ "$ownerEmail" = - ] && ownerEmail=""

readonly app=/home/vmcab/cabinet
readonly bun=/home/vmcab/.bun/bin/bun
readonly access=/srv/vibememory/access/access.json
readonly serverBin=/srv/vibememory/bin/vibememory-mcp
id vmcab >/dev/null 2>&1 || { echo "Ошибка: нет учётки vmcab — сначала hostBootstrap.sh" >&2; exit 1; }
[ -x "$serverBin" ] || { echo "Ошибка: нет $serverBin — сначала hostMcp.sh" >&2; exit 1; }
asCab() { sudo -u vmcab -H bash -c "cd $app && $1"; }

# 2. Packages: Postgres from PGDG (the version the cabinet is tested against), unzip for Bun's
# installer, age for the backup of the database, rsync for the release, openssl for the secrets.
. /etc/os-release
pgdgList=/etc/apt/sources.list.d/pgdg.list
pgdgKey=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc
pgdgLine="deb [signed-by=$pgdgKey] https://apt.postgresql.org/pub/repos/apt $VERSION_CODENAME-pgdg main"
if [ "$(cat "$pgdgList" 2>/dev/null || true)" != "$pgdgLine" ]; then
  sudo install -d /usr/share/postgresql-common/pgdg
  sudo curl -fsSLo "$pgdgKey" https://www.postgresql.org/media/keys/ACCC4CF8.asc
  printf '%s\n' "$pgdgLine" | sudo tee "$pgdgList" >/dev/null
  sudo apt-get update -qq
fi
missing=""
for package in "postgresql-$pgVersion" unzip age rsync openssl; do
  dpkg -s "$package" >/dev/null 2>&1 || missing="$missing $package"
done
if [ -n "$missing" ]; then
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq $missing >/dev/null
  echo "2/9 Пакеты поставлены:$missing"
else
  echo "2/9 Пакеты: без изменений"
fi

# 3. Postgres: local only, small, a connection cap the pools fit under.
pgConf="/etc/postgresql/$pgVersion/main/conf.d/vibememory.conf"
pgText="listen_addresses = 'localhost'
shared_buffers = $sharedBuffers
max_connections = $maxConnections"
if [ "$(sudo cat "$pgConf" 2>/dev/null || true)" != "$pgText" ]; then
  printf '%s\n' "$pgText" | sudo tee "$pgConf" >/dev/null
  sudo systemctl restart "postgresql@$pgVersion-main"
  echo "3/9 Postgres $pgVersion: настройки записаны, служба перезапущена"
else
  echo "3/9 Postgres $pgVersion: без изменений"
fi
sudo systemctl enable --quiet "postgresql@$pgVersion-main"

# 4. The .env: managed values rewritten each run, secrets generated once and kept, the owner's
# own values (Resend, Telegram, the support link) never touched here.
sudo install -d -o vmcab -g vmcab -m 750 "$app"
declare -A env=()
if sudo test -f "$app/.env"; then
  while IFS= read -r line; do
    case "$line" in ''|'#'*) continue ;; esac
    env["${line%%=*}"]="${line#*=}"
  done < <(sudo cat "$app/.env")
fi
secret() { openssl rand -base64 32 | tr -d '\n/+='; }
dbPassword="${env[DATABASE_URL]:-}"
dbPassword="${dbPassword#postgresql://vmcab:}"; dbPassword="${dbPassword%%@*}"
[ -n "$dbPassword" ] && [ "$dbPassword" != "${env[DATABASE_URL]:-}" ] || dbPassword=$(secret)
if [ -n "$ownerEmail" ]; then env[OWNER_EMAIL]="$ownerEmail"; fi
env[HOST_ENV]=prod
env[NODE_ENV]=production
env[PORT]="$cabinetPort"
env[SERVER_URL]="https://$appDomain"
env[CLIENT_URL]="https://$appDomain"
env[SOURCE_VERSION]="$sourceVersion"
env[LOG_MODE]=json
env[LOG_LEVEL]=info
env[DATABASE_URL]="postgresql://vmcab:$dbPassword@localhost:5432/cabinet?schema=public"
env[BETTER_AUTH_SECRET]="${env[BETTER_AUTH_SECRET]:-$(secret)}"
env[OPENAPI_CREDENTIALS]="${env[OPENAPI_CREDENTIALS]:-owner:$(secret)}"
env[GOOGLE_CLIENT_ID]="${env[GOOGLE_CLIENT_ID]:-__XXX__}"
env[GOOGLE_CLIENT_SECRET]="${env[GOOGLE_CLIENT_SECRET]:-__XXX__}"
env[RESEND_API_KEY]="${env[RESEND_API_KEY]:-__XXX__}"
env[FROM_EMAIL_ADDRESS]="${env[FROM_EMAIL_ADDRESS]:-cabinet@$domain}"
env[FROM_EMAIL_NAME]="${env[FROM_EMAIL_NAME]:-VibeMemory}"
env[VIBEMEMORY_MCP_BIN]="$serverBin"
env[ACCESS_DIR]=/srv/vibememory/access
env[RELEASES_DIR]=/srv/vibememory/releases
env[MCP_URL]="https://$domain/mcp"
env[SSH_HOST]="$domain"
envText=$(for key in $(printf '%s\n' "${!env[@]}" | sort); do printf '%s=%s\n' "$key" "${env[$key]}"; done)
envChanged=0
if [ "$(sudo cat "$app/.env" 2>/dev/null || true)" != "$envText" ]; then
  candidate=$(mktemp)
  printf '%s\n' "$envText" > "$candidate"
  sudo install -o vmcab -g vmcab -m 600 "$candidate" "$app/.env"
  rm -f "$candidate"
  envChanged=1
  echo "4/9 .env кабинета записан (0600, vmcab); секреты — на хосте, наружу не печатаются"
else
  echo "4/9 .env кабинета: без изменений"
fi

# 5. Roles and the database: vmcab owns it and signs in with the password from .env; vmgit reads
# everything through peer authentication and has no password at all — the nightly dump is its.
psqlAdmin() { sudo -u postgres psql -qtAX -v ON_ERROR_STOP=1 "$@"; }
if [ -z "$(psqlAdmin -c "select 1 from pg_roles where rolname = 'vmcab'")" ]; then
  psqlAdmin -c "create role vmcab login" >/dev/null
  echo "5/9 Роль vmcab создана"
fi
# the password travels in a here-document, never on a command line
psqlAdmin <<SQL >/dev/null
\set password '$dbPassword'
alter role vmcab password :'password';
SQL
[ -n "$(psqlAdmin -c "select 1 from pg_database where datname = 'cabinet'")" ] ||
  { psqlAdmin -c "create database cabinet owner vmcab" >/dev/null; echo "5/9 База cabinet создана"; }
[ -n "$(psqlAdmin -c "select 1 from pg_roles where rolname = 'vmgit'")" ] ||
  { psqlAdmin -c "create role vmgit login" >/dev/null; echo "5/9 Роль vmgit создана"; }
psqlAdmin -c "grant pg_read_all_data to vmgit" >/dev/null
psqlAdmin -c "grant connect on database cabinet to vmgit" >/dev/null
echo "5/9 Роли: vmcab — владелец базы, vmgit — только чтение по peer"

# 6. Bun under vmcab, the version the cabinet is tested with.
if [ "$(sudo -u vmcab "$bun" --version 2>/dev/null || true)" != "$bunVersion" ]; then
  sudo -u vmcab -H bash -c "curl -fsSL https://bun.sh/install | bash -s bun-v$bunVersion" >/dev/null
  sudo -u vmcab -H "$bun" pm cache rm >/dev/null 2>&1 || true
  echo "6/9 Bun $bunVersion поставлен под vmcab"
else
  echo "6/9 Bun $bunVersion: без изменений"
fi

# 7. The release: dist/ replaced whole, the manifests and migrations beside it; production
# dependencies only, and the migrations applied with nothing but them.
releaseChanged=0
if [ "$(sudo cat "$app/.release" 2>/dev/null || true)" != "$releaseHash" ]; then
  staging=$(mktemp -d)
  tar -xzf "$HOME/cabinet-release.tar.gz" -C "$staging"
  sudo rsync -a --delete --chown=vmcab:vmcab "$staging/dist/" "$app/dist/"
  sudo rsync -a --delete --chown=vmcab:vmcab "$staging/src/modules/prisma/" "$app/src/modules/prisma/"
  for file in package.json bun.lock prisma.config.ts; do
    sudo install -o vmcab -g vmcab -m 640 "$staging/$file" "$app/$file"
  done
  rm -rf "$staging"
  asCab "$bun install --production --frozen-lockfile >/dev/null"
  printf '%s\n' "$releaseHash" | sudo tee "$app/.release" >/dev/null
  releaseChanged=1
  echo "7/9 Выложена сборка $sourceVersion, зависимости — только production"
else
  echo "7/9 Сборка $sourceVersion: без изменений"
fi
rm -f "$HOME/cabinet-release.tar.gz"
migrated=$(asCab "NODE_ENV=production $bun x prisma migrate deploy 2>&1") ||
  { printf '%s\n' "$migrated" >&2; echo "Ошибка: миграции не применились" >&2; exit 1; }
case "$migrated" in
  *"No pending migrations"*) echo "7/9 Миграции: без изменений" ;;
  *) echo "7/9 Миграции применены" ;;
esac

# 8. The owner, then the service. The import reads the host's own snapshot — the one the memory
# server runs on — so the first publication says what the host already knows.
if ! asCab "NODE_ENV=production $bun run access:seeded >/dev/null 2>&1"; then
  if [ -n "${env[OWNER_EMAIL]:-}" ]; then
    asCab "NODE_ENV=production $bun run access:import $access"
    echo "8/9 Владелец импортирован из $access"
  else
    echo "8/9 Владельца в базе нет, адреса владельца нет: импорт ждёт --owner-email, служба не стартует"
  fi
fi
unit=/etc/systemd/system/vibememory-cabinet.service
unitText="[Unit]
Description=VibeMemory cabinet (behind Caddy)
After=network.target postgresql@$pgVersion-main.service
Requires=postgresql@$pgVersion-main.service

[Service]
User=vmcab
WorkingDirectory=$app
Environment=NODE_ENV=production
# A cabinet on a database without its owner does not start: it would publish an empty snapshot.
ExecStartPre=$bun run access:seeded
ExecStart=$bun run start
Restart=on-failure
RestartSec=5
UMask=0027
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/srv/vibememory/access /home/vmcab

[Install]
WantedBy=multi-user.target"
unitChanged=0
if [ "$(sudo cat "$unit" 2>/dev/null || true)" != "$unitText" ]; then
  printf '%s\n' "$unitText" | sudo tee "$unit" >/dev/null
  sudo systemctl daemon-reload
  unitChanged=1
fi
sudo systemctl enable --quiet vibememory-cabinet
if ! asCab "NODE_ENV=production $bun run access:seeded >/dev/null 2>&1"; then
  echo "8/9 Служба vibememory-cabinet включена, но не запущена: нет владельца"
elif [ "$releaseChanged$envChanged$unitChanged" != 000 ] || ! systemctl is-active --quiet vibememory-cabinet; then
  sudo systemctl restart vibememory-cabinet
  healthy=0
  for _ in $(seq 1 60); do
    [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$cabinetPort/api/health")" = 200 ] && { healthy=1; break; }
    sleep 1
  done
  [ "$healthy" = 1 ] || { echo "Ошибка: кабинет не ответил на /api/health — journalctl -u vibememory-cabinet" >&2; exit 1; }
  echo "8/9 Служба vibememory-cabinet запущена, /api/health — 200"
else
  echo "8/9 Служба vibememory-cabinet: без изменений ($(systemctl is-active vibememory-cabinet))"
fi

# 9. The journal's size and the cabinet's jail. The cabinet writes one line per refused password or
# claim with the address Caddy saw (the last X-Forwarded-For entry); the unit's lines land in a
# user journal, so the backend needs journalflags=1, and the filter is not anchored at ^ — both
# measured on the memory server's jail (hostMcp.sh).
journald=/etc/systemd/journald.conf.d/vibememory.conf
journaldText="[Journal]
SystemMaxUse=$journalMaxUse"
if [ "$(sudo cat "$journald" 2>/dev/null || true)" != "$journaldText" ]; then
  sudo install -d /etc/systemd/journald.conf.d
  printf '%s\n' "$journaldText" | sudo tee "$journald" >/dev/null
  sudo systemctl restart systemd-journald
  echo "9/9 journald: не больше $journalMaxUse"
fi
if ! command -v fail2ban-client >/dev/null && [ ! -x /usr/bin/fail2ban-client ]; then
  echo "9/9 fail2ban не установлен — джейл кабинета пропущен"
  exit 0
fi
filter=/etc/fail2ban/filter.d/vibememory-cabinet.conf
filterText="[Definition]
failregex = (?:sign-in|claim) refused from <HOST>
journalmatch = _SYSTEMD_UNIT=vibememory-cabinet.service"
jail=/etc/fail2ban/jail.d/vibememory-cabinet.local
jailText="[vibememory-cabinet]
enabled = true
filter = vibememory-cabinet
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
if [ "$changed" = 1 ]; then
  # restart, not reload: a reload that changes a jail's action leaves it with none (fail2ban 1.1)
  sudo systemctl restart fail2ban
  for _ in $(seq 1 20); do sudo /usr/bin/fail2ban-client ping >/dev/null 2>&1 && break; sleep 1; done
fi
actions=$(sudo /usr/bin/fail2ban-client get vibememory-cabinet actions 2>/dev/null | tail -n +2)
if [ -z "$actions" ]; then
  echo "9/9 fail2ban: джейл vibememory-cabinet БЕЗ действия — баны не дойдут до firewall" >&2
  exit 1
fi
[ "$changed" = 1 ] && echo "9/9 fail2ban: джейл vibememory-cabinet с действием $actions" ||
  echo "9/9 fail2ban: без изменений"
REMOTE

# Caddy last, from the template the memory server's script uses too.
"$(dirname "$0")/caddyApply.sh" --alias "$sshAlias" --domain "$domain" --app-domain "$appDomain" \
  --cabinet-port "$CABINET_PORT" --mcp-port "$MCP_PORT"
