#!/usr/bin/env bash
# The nightly dump of a cabinet's database, encrypted before it touches the disk.
#
#   cabinetDump.sh [<database>]       the memory cabinet's `cabinet` when none is named
#
# Runs ON THE HOST as vmdump from a dump unit the backup pulls in and waits for: vibememory-dump.service
# for the memory cabinet (hostCabinet.sh puts it and the drop-in beside vibememory-backup.service), and
# borodatych-dump.service for the owner's order cabinet (ownerCabinetBackup.sh). vmdump is the one
# account with a role that reads the databases — peer authentication, read-only rights, no password
# anywhere — and it writes nothing but ciphertext for the owner's age key. The backup, under vmgit,
# takes the encrypted files as they are: the account that faces the network through git and the
# memory server never reads a database and never holds a plain dump.
set -euo pipefail

readonly DEFAULT_DATABASE=cabinet
readonly RECIPIENT=/srv/vibememory/backup/recipient.txt
readonly DUMP_DIR=/srv/vibememory/backup/dump
readonly TAG=vibememory-dump

say() { logger -t "$TAG" -- "$*"; }
fail() { say "failed: $*"; printf 'cabinetDump.sh: %s\n' "$*" >&2; exit 1; }

database="${1:-$DEFAULT_DATABASE}"
# the name becomes a file name and a connection target: nothing but a plain name, and never an option
case "$database" in *[!a-z0-9_-]*|-*|"") fail "not a database name: $database" ;; esac
out="$DUMP_DIR/$database.pgdump.age"

[ -s "$RECIPIENT" ] || fail "no recipient in $RECIPIENT"
command -v age >/dev/null || fail "age is not installed"

partial="$out.partial"
trap 'rm -f "$partial"' EXIT
# pipefail: a dump that breaks off fails the pipe, and the last good file stays in place
pg_dump --format=custom --dbname="$database" | age -R "$RECIPIENT" -o "$partial" ||
  fail "dump of the $database database"
chmod 640 "$partial"
mv -f "$partial" "$out"
say "dumped the $database database: $(stat -c %s "$out") bytes, encrypted"
