#!/usr/bin/env bash
# Nightly encrypted backup of what exists nowhere else: the stores of the memory teams, whose
# members keep no clone (a sync team is backed up by every member's clone), the memory cabinet's
# database and the database of the owner's order cabinet.
#
#   hostBackup.sh <remote>          e.g. memoryBackup:VibeBrains/VibeMemoryBackup.git
#
# Runs ON THE HOST as vmgit from vibememory-backup.timer; installed by infra/backupSetup.sh.
#
# Each store becomes `git bundle --all`, encrypted with age to the owner's public key, and the
# night's files become one commit without history, pushed with --force over the backup's deploy
# key: the backup repository does not grow, and only the owner's key — kept on the owner's Mac —
# opens it. The host cannot read its own backups.
#
# The list of memory teams comes from the access snapshot through the server binary, which checks
# the snapshot first: a backup of a guessed list would look complete and not be. Each database comes
# dumped and encrypted by vmdump, which runs just before (a dump unit per database, cabinetDump.sh):
# this account has no role in any database and takes the ciphertext as it is. A host without a
# cabinet has nothing of it to dump; a host with it and no dump wired in fails the backup, and so
# does a dump that is not this night's.
set -euo pipefail

readonly SERVER_BIN=/srv/vibememory/bin/vibememory-mcp
readonly ACCESS=/srv/vibememory/access/access.json
readonly TEAMS=/srv/vibememory/teams
readonly RECIPIENT=/srv/vibememory/backup/recipient.txt
# Read by the host's report: the cabinet raises an alarm when the backup is older than a day.
readonly REPORT=/srv/vibememory/access/backup.json
readonly TAG=vibememory-backup
# The databases, one line each: the name, the variable the backup's drop-in names its dump with, the
# unit of the cabinet that owns it — with that unit on the host, the backup is incomplete without the
# dump — and the script that wires the dump in.
readonly DATABASES=(
  "cabinet VIBEMEMORY_DUMP /etc/systemd/system/vibememory-cabinet.service hostCabinet.sh"
  "borodatych BORODATYCH_DUMP /etc/systemd/system/borodatych-cabinet.service ownerCabinetBackup.sh"
  "borodatych-terms BORODATYCH_TERMS_DUMP /etc/systemd/system/borodatych-terms.service ownerCabinetBackup.sh"
)
# The dumps run right before the backup; one older than this is a night that did not dump.
readonly DUMP_MAX_AGE_MINUTES=60

say() { logger -t "$TAG" -- "$*"; }
fail() { say "failed: $*"; printf 'hostBackup.sh: %s\n' "$*" >&2; exit 1; }

remote="${1:-}"
[ -n "$remote" ] || fail "usage: hostBackup.sh <remote>"
[ -s "$RECIPIENT" ] || fail "no recipient in $RECIPIENT"
command -v age >/dev/null || fail "age is not installed"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
out="$work/backup"
mkdir -p "$out"

dumped=""
for line in "${DATABASES[@]}"; do
  read -r database variable unit wiring <<<"$line"
  dump="${!variable:-}"
  if [ -n "$dump" ]; then
    [ -s "$dump" ] || fail "no dump of the $database database at $dump"
    [ -n "$(find "$dump" -mmin "-$DUMP_MAX_AGE_MINUTES")" ] ||
      fail "the dump at $dump is older than $DUMP_MAX_AGE_MINUTES minutes"
    cp "$dump" "$out/$database.pgdump.age"
    dumped="${dumped:+$dumped }$database"
  elif [ -e "$unit" ]; then
    fail "the cabinet of the $database database is on this host and its dump is not wired into the backup: run $wiring"
  fi
done

slugs=$("$SERVER_BIN" access teams "$ACCESS" --mode memory) || fail "the access snapshot cannot be used"
count=0
for slug in $slugs; do
  repo="$TEAMS/$slug.git"
  # A store the application has not created yet has nothing to lose.
  [ -d "$repo" ] || continue
  [ -n "$(git --git-dir="$repo" for-each-ref --count=1)" ] || continue
  git --git-dir="$repo" bundle create --quiet "$work/$slug.bundle" --all || fail "bundle of $slug"
  age -R "$RECIPIENT" -o "$out/$slug.bundle.age" "$work/$slug.bundle" || fail "encryption of $slug"
  rm -f "$work/$slug.bundle"
  count=$((count + 1))
done

cat > "$out/README.md" <<'TEXT'
# Бэкап VibeMemory: сторы memory-команд и базы кабинетов

Каждую ночь хост кладёт сюда один коммит без истории.
В нём сторы memory-команд: у их участников клона нет, и других копий не существует.
Каждый `<слаг>.bundle.age` — `git bundle --all` стора, зашифрованный age открытым ключом владельца.
`cabinet.pgdump.age` — дамп базы кабинета VibeMemory (`pg_dump --format=custom`) тем же ключом.
`borodatych.pgdump.age` — так же дамп базы кабинета заказов app.borodatych.ru.

Восстановить стор на Mac владельца:

```
age -d -i ~/.vibememory/keys/backup.age <слаг>.bundle.age > <слаг>.bundle
git clone --bare <слаг>.bundle <слаг>.git
```

Восстановить базу кабинета VibeMemory — порядок и что делать после дампа описаны в docs/manuals/cabinetSetup.md.
Базу кабинета заказов — в docs/manuals/deploy.md репозитория BorodatychCabinet.

```
age -d -i ~/.vibememory/keys/backup.age cabinet.pgdump.age > cabinet.pgdump
pg_restore --clean --if-exists --no-owner --dbname=<база> cabinet.pgdump
```

Закрытый ключ есть только у владельца.
Без него файлы не открываются — в том числе на самом хосте.
TEXT

now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
git -C "$out" init --quiet --initial-branch=main
git -C "$out" add -A
git -C "$out" -c user.name=vibememory-backup -c user.email=backup@vibememory.invalid \
  -c commit.gpgsign=false commit --quiet -m "backup $now: $count memory stores, databases: ${dumped:-none}"
git -C "$out" push --quiet --force "$remote" main || fail "push to $remote"

printf '{\n  "lastAt": "%s"\n}\n' "$now" > "$REPORT.new"
chmod 640 "$REPORT.new"
mv -f "$REPORT.new" "$REPORT"
say "done: $count memory stores, databases: ${dumped:-none}, $(du -sb --exclude=.git "$out" | cut -f1) bytes"
