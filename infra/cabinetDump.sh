#!/usr/bin/env bash
# The nightly dump of the cabinet's database, encrypted before it touches the disk.
#
#   cabinetDump.sh
#
# Runs ON THE HOST as vmdump from vibememory-dump.service, which the backup pulls in and waits for
# (the drop-in hostCabinet.sh puts beside vibememory-backup.service). vmdump is the one account with
# a role that reads the database — peer authentication, read-only rights, no password anywhere — and
# it writes nothing but ciphertext for the owner's age key. The backup, under vmgit, takes the
# encrypted file as it is: the account that faces the network through git and the memory server
# never reads the database and never holds a plain dump.
set -euo pipefail

readonly DATABASE=cabinet
readonly RECIPIENT=/srv/vibememory/backup/recipient.txt
readonly OUT=/srv/vibememory/backup/dump/cabinet.pgdump.age
readonly TAG=vibememory-dump

say() { logger -t "$TAG" -- "$*"; }
fail() { say "failed: $*"; printf 'cabinetDump.sh: %s\n' "$*" >&2; exit 1; }

[ -s "$RECIPIENT" ] || fail "no recipient in $RECIPIENT"
command -v age >/dev/null || fail "age is not installed"

partial="$OUT.partial"
trap 'rm -f "$partial"' EXIT
# pipefail: a dump that breaks off fails the pipe, and the last good file stays in place
pg_dump --format=custom --dbname="$DATABASE" | age -R "$RECIPIENT" -o "$partial" ||
  fail "dump of the $DATABASE database"
chmod 640 "$partial"
mv -f "$partial" "$OUT"
say "dumped the $DATABASE database: $(stat -c %s "$OUT") bytes, encrypted"
