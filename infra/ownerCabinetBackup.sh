#!/usr/bin/env bash
# Wires the database of the owner's order cabinet (BorodatychCabinet, app.borodatych.ru) into the
# host's nightly encrypted backup, the way hostCabinet.sh wires the memory cabinet's: vmdump dumps it
# through peer authentication and encrypts it for the owner's age key in borodatych-dump.service, and
# the backup pulls that unit in, waits for it and takes the ciphertext (a drop-in beside its unit).
#
# Runs on the OWNER'S machine; the order cabinet's infra/hostCabinet.sh calls it by path after it has
# made the database, and it can be run alone. Idempotent: the grant, the unit and the drop-in change
# only where they differ. Every run dumps the database once, so a run that passes proves the chain.
#
# It writes none of what the memory cabinet's scripts own, and checks it instead: vmdump and
# cabinetDump.sh come from hostCabinet.sh, the backup script from backupSetup.sh. A host where either
# is older than this wiring needs fails here, with the script to run, rather than back up without
# the database.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DATABASE=borodatych
# The cluster hostCabinet.sh puts and tunes; the dump waits for it, as the memory cabinet's does.
readonly PG_VERSION=17
# The backup's variable naming this dump: hostBackup.sh reads it, and fails when the order cabinet's
# unit is on the host and the variable is not set.
readonly DUMP_VARIABLE=BORODATYCH_DUMP
# A lost SYN is retried instead of failing the run: the path to the host drops a connection now and
# then, and every step here is safe to repeat.
readonly SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=15 -o ConnectionAttempts=4)

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"

fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --alias) sshAlias="${2:-}"; shift 2 ;;
    -h | --help)
      printf 'Подключить базу кабинета заказов (%s) к ночному шифрованному бэкапу хоста.\n\n  ./infra/ownerCabinetBackup.sh [--alias vibememory]\n' "$DATABASE"
      exit 0 ;;
    *) fail "неизвестный аргумент $1 (--help покажет список)" ;;
  esac
done

# shellcheck disable=SC2029 # the arguments are quoted for the remote shell on purpose
ssh "${SSH_OPTIONS[@]}" "$sshAlias" "bash -s -- $(printf '%q ' "$DATABASE" "$DUMP_VARIABLE" "$PG_VERSION")" <<'REMOTE'
set -euo pipefail
database="$1"; dumpVariable="$2"; pgVersion="$3"
readonly dumpScript=/srv/vibememory/bin/cabinetDump.sh
readonly backupScript=/srv/vibememory/bin/hostBackup.sh
readonly dumpDir=/srv/vibememory/backup/dump
dump="$dumpDir/$database.pgdump.age"
unitName="$database-dump.service"
psqlAdmin() { sudo -u postgres psql -qtAX -v ON_ERROR_STOP=1 "$@"; }

# What the memory cabinet's scripts put, checked rather than written: two writers of one file would
# undo each other.
if ! id vmdump >/dev/null 2>&1 || ! sudo test -d "$dumpDir"; then
  echo "Ошибка: нет учётки vmdump или каталога дампов — в VibeMemory: ./infra/hostCabinet.sh" >&2; exit 1
fi
sudo grep -q DEFAULT_DATABASE "$dumpScript" 2>/dev/null ||
  { echo "Ошибка: $dumpScript снимает только базу cabinet — в VibeMemory: ./infra/hostCabinet.sh" >&2; exit 1; }
sudo grep -q "$dumpVariable" "$backupScript" 2>/dev/null ||
  { echo "Ошибка: скрипт бэкапа на хосте не знает базу $database — в VibeMemory: ./infra/backupSetup.sh" >&2; exit 1; }
[ -n "$(psqlAdmin -c "select 1 from pg_database where datname = '$database'")" ] ||
  { echo "Ошибка: базы $database нет — сначала infra/hostCabinet.sh в BorodatychCabinet" >&2; exit 1; }

changed=0
# vmdump reads every database through pg_read_all_data; connecting is granted per database, since
# the order cabinet's script revokes it from PUBLIC
if [ "$(psqlAdmin -c "select has_database_privilege('vmdump', '$database', 'connect')")" != t ]; then
  psqlAdmin -c "grant connect on database $database to vmdump" >/dev/null
  echo "Бэкап базы $database: vmdump может подключиться"
fi

unit="/etc/systemd/system/$unitName"
unitText="[Unit]
Description=The owner's order cabinet: its database, dumped and encrypted for the nightly backup
After=postgresql@$pgVersion-main.service
Requires=postgresql@$pgVersion-main.service

[Service]
Type=oneshot
User=vmdump
UMask=0027
ExecStart=$dumpScript $database
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=$dumpDir"
dropIn="/etc/systemd/system/vibememory-backup.service.d/$database-dump.conf"
dropInText="[Unit]
Requires=$unitName
After=$unitName

[Service]
Environment=$dumpVariable=$dump"
if [ "$(sudo cat "$unit" 2>/dev/null || true)" != "$unitText" ]; then
  printf '%s\n' "$unitText" | sudo tee "$unit" >/dev/null
  changed=1
fi
if [ "$(sudo cat "$dropIn" 2>/dev/null || true)" != "$dropInText" ]; then
  sudo install -d /etc/systemd/system/vibememory-backup.service.d
  printf '%s\n' "$dropInText" | sudo tee "$dropIn" >/dev/null
  changed=1
fi
if [ "$changed" = 1 ]; then
  sudo systemctl daemon-reload
  echo "Бэкап базы $database: $unitName под vmdump перед каждым бэкапом"
else
  echo "Бэкап базы $database: юнит и drop-in без изменений"
fi

# One dump now: the night would be the first to find a broken chain otherwise.
sudo systemctl start "$unitName" ||
  { echo "Ошибка: дамп базы $database не снялся — journalctl -t vibememory-dump" >&2; exit 1; }
if ! sudo test -s "$dump" || [ -z "$(sudo find "$dump" -mmin -5)" ]; then
  echo "Ошибка: после дампа нет свежего $dump" >&2; exit 1
fi
echo "Бэкап базы $database: дамп снят и зашифрован, $(sudo stat -c %s "$dump") байт"
REMOTE
