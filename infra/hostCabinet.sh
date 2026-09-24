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
# The nightly dump of the database, run on the host by vmdump before the backup.
readonly DUMP_SCRIPT_LOCAL="$(cd "$(dirname "$0")" && pwd)/cabinetDump.sh"
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
# A lost SYN is retried instead of failing the run: the path to the host drops a connection now and
# then, and every step here is safe to repeat.
readonly SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=15 -o ConnectionAttempts=4)

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
ssh -n "${SSH_OPTIONS[@]}" "$sshAlias" 'echo ok' >/dev/null 2>&1 || fail "сервер не пускает по ключу"

# 1. The release: built here, packed with exactly what runs it.
sourceVersion=$(git -C "$CABINET_LOCAL" rev-parse --short=12 HEAD)
[ -z "$(git -C "$CABINET_LOCAL" status --porcelain -- .)" ] ||
  say "   внимание: в cabinet/ есть незакоммиченные правки — SOURCE_VERSION их не описывает"
mkdir -p "$RELEASES_LOCAL"
release="$RELEASES_LOCAL/cabinet-$sourceVersion.tar.gz"
if [ "$skipBuild" = 0 ]; then
  say "1/9 Собираю кабинет ($sourceVersion)"
  (cd "$CABINET_LOCAL" && bun run build:code >/dev/null 2>&1) || fail "сборка кабинета не прошла: bun run build:code в cabinet/"
  # the client's source maps stay here, as the pack's Dockerfile drops them: dist/client is served
  # to anyone, and the server keeps its own maps for readable stack traces in the journal
  # no macOS extended attributes: GNU tar on the host warns on every one of them
  COPYFILE_DISABLE=1 tar --no-xattrs -czf "$release" -C "$CABINET_LOCAL" \
    --exclude='dist/client/*.map' --exclude='dist/client/**/*.map' \
    dist package.json bun.lock prisma.config.ts src/modules/prisma/schema.prisma src/modules/prisma/migrations
else
  [ -f "$release" ] || fail "нет сборки $release — запустите без --skip-build"
  say "1/9 Сборка $sourceVersion взята готовой"
fi
# The release is what it holds, not the archive's bytes: tar and gzip stamp every build with its own
# times, while the build itself is deterministic — the same commit gives the same files, and a run
# with nothing new neither replaces the release nor restarts the service.
releaseHash=$(
  tree=$(mktemp -d)
  tar -xzf "$release" -C "$tree"
  (cd "$tree" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256) | shasum -a 256 | cut -d ' ' -f 1
  rm -rf "$tree"
)
# rsync with --partial: a transfer the network cuts resumes where it stopped instead of starting over
for attempt in 1 2 3 4 5; do
  rsync -q --partial --timeout=60 -e "ssh ${SSH_OPTIONS[*]}" "$release" "$DUMP_SCRIPT_LOCAL" "$sshAlias:" && break
  [ "$attempt" -lt 5 ] || fail "архив сборки не доехал до хоста за пять попыток"
  say "   передача оборвалась, повтор $((attempt + 1))/5"
done
ssh -n "${SSH_OPTIONS[@]}" "$sshAlias" "mv -f $(printf '%q' "$(basename "$release")") cabinet-release.tar.gz"

ssh "${SSH_OPTIONS[@]}" "$sshAlias" "bash -s -- $(printf '%q ' "$domain" "$appDomain" "$sourceVersion" \
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
# the umbrella unit too: the host's report asks `postgresql`, and an inactive umbrella reads as a fault
sudo systemctl enable --now --quiet postgresql

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
# loopback only: the cabinet is reached through Caddy, never directly
env[SERVER_HOSTNAME]=127.0.0.1
env[SERVER_URL]="https://$appDomain"
env[CLIENT_URL]="https://$appDomain"
env[SOURCE_VERSION]="$sourceVersion"
env[LOG_MODE]=json
env[LOG_LEVEL]=info
# every query at info would fill the journal; their parameters are never logged in prod anyway
env[LOG_FILTER]='*,-prisma'
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

# 5. Roles and the database: vmcab owns it and signs in with the password from .env; vmdump alone
# reads it, through peer authentication and without a password, to dump it for the nightly backup
# encrypted before it touches the disk (cabinetDump.sh). vmgit, which faces the network through git
# and the memory server, has no role at all, and nobody else may connect: a hole in either of them
# reads no database.
psqlAdmin() { sudo -u postgres psql -qtAX -v ON_ERROR_STOP=1 "$@"; }
rolesChanged=0
if [ -z "$(psqlAdmin -c "select 1 from pg_roles where rolname = 'vmcab'")" ]; then
  psqlAdmin -c "create role vmcab login" >/dev/null
  echo "5/9 Роль vmcab создана"
  rolesChanged=1
fi
# The password from .env is set only when it does not already sign vmcab in: the check signs in
# through a passfile readable by root alone, removed at once, and the password travels in a
# here-document, never on a command line.
passfile=$(sudo mktemp)
printf 'localhost:5432:postgres:vmcab:%s\n' "$dbPassword" | sudo tee "$passfile" >/dev/null
if ! sudo psql "host=localhost port=5432 dbname=postgres user=vmcab passfile=$passfile" -qtAc 'select 1' >/dev/null 2>&1; then
  psqlAdmin <<SQL >/dev/null
\set password '$dbPassword'
alter role vmcab password :'password';
SQL
  echo "5/9 Пароль vmcab поставлен из .env"
  rolesChanged=1
fi
sudo rm -f "$passfile"
[ -n "$(psqlAdmin -c "select 1 from pg_database where datname = 'cabinet'")" ] ||
  { psqlAdmin -c "create database cabinet owner vmcab" >/dev/null; echo "5/9 База cabinet создана"; rolesChanged=1; }
if ! id vmdump >/dev/null 2>&1; then
  sudo useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin vmdump
  echo "5/9 Учётка vmdump создана"
  rolesChanged=1
fi
[ -n "$(psqlAdmin -c "select 1 from pg_roles where rolname = 'vmdump'")" ] ||
  { psqlAdmin -c "create role vmdump login" >/dev/null; echo "5/9 Роль vmdump создана"; rolesChanged=1; }
if [ "$(psqlAdmin -c "select pg_has_role('vmdump', 'pg_read_all_data', 'member')")" != t ]; then
  psqlAdmin -c "grant pg_read_all_data to vmdump" >/dev/null
  rolesChanged=1
fi
if [ "$(psqlAdmin -c "select has_database_privilege('vmdump', 'cabinet', 'connect')")" != t ]; then
  psqlAdmin -c "grant connect on database cabinet to vmdump" >/dev/null
  rolesChanged=1
fi
# Postgres lets every role connect to a new database; here only the owner and vmdump do.
if [ "$(psqlAdmin -c "select has_database_privilege('public', 'cabinet', 'connect')")" = t ]; then
  psqlAdmin -c "revoke connect on database cabinet from public" >/dev/null
  rolesChanged=1
fi
if [ "$rolesChanged" = 1 ]; then
  echo "5/9 Роли: vmcab — владелец базы, vmdump — только чтение по peer для дампа"
else
  echo "5/9 Роли и база: без изменений"
fi

# The dump: vmdump writes the encrypted file where vmgit — the backup's account — reads it and
# nothing else, and the backup pulls the dump in and waits for it (a drop-in beside its unit).
dumpChanged=0
sudo install -d -o root -g root -m 755 /srv/vibememory/backup
sudo install -d -o vmdump -g vmgit -m 2750 /srv/vibememory/backup/dump
if ! sudo cmp -s "$HOME/cabinetDump.sh" /srv/vibememory/bin/cabinetDump.sh; then
  sudo install -o root -g root -m 755 "$HOME/cabinetDump.sh" /srv/vibememory/bin/cabinetDump.sh
  dumpChanged=1
fi
rm -f "$HOME/cabinetDump.sh"
dumpUnit=/etc/systemd/system/vibememory-dump.service
dumpUnitText="[Unit]
Description=VibeMemory: the cabinet's database, dumped and encrypted for the nightly backup
After=postgresql@$pgVersion-main.service
Requires=postgresql@$pgVersion-main.service

[Service]
Type=oneshot
User=vmdump
UMask=0027
ExecStart=/srv/vibememory/bin/cabinetDump.sh
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/srv/vibememory/backup/dump"
dropIn=/etc/systemd/system/vibememory-backup.service.d/cabinet-dump.conf
dropInText="[Unit]
Requires=vibememory-dump.service
After=vibememory-dump.service

[Service]
Environment=VIBEMEMORY_DUMP=/srv/vibememory/backup/dump/cabinet.pgdump.age"
if [ "$(sudo cat "$dumpUnit" 2>/dev/null || true)" != "$dumpUnitText" ]; then
  printf '%s\n' "$dumpUnitText" | sudo tee "$dumpUnit" >/dev/null
  dumpChanged=1
fi
if [ "$(sudo cat "$dropIn" 2>/dev/null || true)" != "$dropInText" ]; then
  sudo install -d /etc/systemd/system/vibememory-backup.service.d
  printf '%s\n' "$dropInText" | sudo tee "$dropIn" >/dev/null
  dumpChanged=1
fi
if [ "$dumpChanged" = 1 ]; then
  sudo systemctl daemon-reload
  echo "5/9 Дамп базы: vibememory-dump.service под vmdump перед каждым бэкапом"
else
  echo "5/9 Дамп базы: без изменений"
fi
# vmgit's role goes once nothing dumps with it: a backup script from before vmdump reads the database
# itself, and without the role it would leave the database out of the backup without a word.
backupScript=/srv/vibememory/bin/hostBackup.sh
if [ -n "$(psqlAdmin -c "select 1 from pg_roles where rolname = 'vmgit'")" ]; then
  if ! sudo test -e "$backupScript" || sudo grep -q VIBEMEMORY_DUMP "$backupScript"; then
    psqlAdmin -c "revoke all on database cabinet from vmgit" -c "drop role vmgit" >/dev/null
    echo "5/9 Роль vmgit снята: базу читает только vmdump"
  else
    echo "5/9 Роль vmgit оставлена: бэкап на хосте ещё снимает дамп ею — ./infra/backupSetup.sh, затем этот скрипт ещё раз"
  fi
fi

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
  sudo -u vmcab mkdir -p "$app/dist" "$app/src/modules/prisma"
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

# The cabinet must listen on loopback alone: a port open to the world bypasses TLS, Caddy and the
# X-Forwarded-For the jail trusts. Checked every run, because the default of the server is 0.0.0.0.
if systemctl is-active --quiet vibememory-cabinet; then
  listening=$(sudo ss -ltnH "sport = :$cabinetPort" | awk '{print $4}' | sort -u)
  case "$listening" in
    127.0.0.1:"$cabinetPort") ;;
    *) echo "Ошибка: кабинет слушает не только loopback: $(printf '%s ' $listening)" >&2; exit 1 ;;
  esac
fi

# 9. The journal's size and the cabinet's jail. The cabinet writes one plain line to stderr per
# refused password — at sign-in, or the current one when changing it — claim code, gift code or
# invitation link, with the address Caddy saw (the last
# X-Forwarded-For entry, an IP literal or no line at all). The filter is anchored at the line's end:
# the JSON lines of the journal carry what a request brought — its user agent, its path — and a
# match inside one would let a request pick whom to ban; usedns = no keeps a name from being
# resolved into someone's address. The unit's lines land in a user journal, so the backend needs
# journalflags=1, and the backend prefixes host and process, so no ^ — both measured on the memory
# server's jail (hostMcp.sh).
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
failregex = vibememory-cabinet: (?:sign-in|password|claim|code|invitation) refused from <HOST>\$
journalmatch = _SYSTEMD_UNIT=vibememory-cabinet.service"
jail=/etc/fail2ban/jail.d/vibememory-cabinet.local
jailText="[vibememory-cabinet]
enabled = true
filter = vibememory-cabinet
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
