#!/usr/bin/env bash
# Nightly incremental repack of the stores on the host: the owner's store from the owner's crontab,
# every live team store from the host's timer under vmgit.
#
#   storeRepack.sh <repository>
#   storeRepack.sh --teams <directory>    every <slug>.git in it; renamed deleted stores are frozen
#
# Installed on the host by infra/hostBootstrap.sh as /srv/vibememory/bin/storeRepack.sh.
#
# Why a repack and not `git gc`. `gc.auto=0` keeps garbage collection out of the push path — a tick
# must not wait for a repack — but every push lands as its own pack (`unpackLimit=1`), and hundreds
# of small packs slow every read. `git gc` rewrites the whole store into one new pack, so it needs
# as much free space as the store occupies: on 2026-09-16 it did that once, and on 2026-09-17 it
# died half-way at 2.23 GiB with the disk full, and pushes stopped. `repack --geometric=2` merges
# the small packs and leaves the big one alone while the small ones stay small.
#
# Loose objects the memory server writes go into the new pack too, and only objects already in a
# pack are removed afterwards, so a write that lands during the repack is not lost.
#
# The script refuses to start without room for the worst case — every pack rewritten plus a
# reserve — and says so in the journal: `journalctl -t vibememory-repack`.
#
# `set -e` is left out on purpose: one store that fails must not stop the others, and each failure
# is written to the journal instead.
set -uo pipefail

readonly RESERVE_BYTES=$((1024 * 1024 * 1024))
readonly TAG=vibememory-repack

say() { logger -t "$TAG" -- "$*"; }

repack() {
  local repo="$1" packs packBytes freeBytes needBytes leftover out
  packs="$repo/objects/pack"
  [ -d "$packs" ] || { say "no pack directory in $repo"; return 1; }
  # printf "%.0f", not print: awk prints large sums in exponent form, and under a Russian locale
  # with a decimal comma ("3,63136e+09"), which bash arithmetic rejects. Found on the first live run.
  packBytes=$(find "$packs" -maxdepth 1 -name "pack-*.pack" -printf "%s\n" | LC_ALL=C awk "{s+=\$1} END {printf \"%.0f\", s}")
  freeBytes=$(df -B1 --output=avail "$packs" | tail -1 | tr -d " ")
  needBytes=$((packBytes + RESERVE_BYTES))
  if [ "$freeBytes" -lt "$needBytes" ]; then
    say "$repo skipped: $freeBytes bytes free, $needBytes needed (packs $packBytes + reserve $RESERVE_BYTES)"
    return 1
  fi
  leftover=$(find "$packs" -maxdepth 1 -name "tmp_pack_*" | wc -l)
  [ "$leftover" -eq 0 ] || say "$repo warning: $leftover tmp_pack file(s) left by an earlier run"
  # --git-dir, not -C: the repository is shared with another account, and -C checks ownership.
  # -n: no `update-server-info`. It rewrites info/refs on every run for the dumb HTTP transport,
  # which no store here serves, and writes it group-writable outside objects/ and refs/.
  if out=$(git --git-dir="$repo" repack -d -n --geometric=2 --quiet 2>&1); then
    say "$repo done: $(git --git-dir="$repo" count-objects -v | tr "\n" " ")"
  else
    say "$repo failed: $out"
    return 1
  fi
}

case "${1:-}" in
  --teams)
    dir="${2:?--teams needs the directory of the team stores}"
    failed=0
    for repo in "$dir"/*.git; do
      [ -d "$repo" ] || continue
      case "$(basename "$repo")" in *.deleted-*) continue ;; esac
      repack "$repo" || failed=1
    done
    exit "$failed"
    ;;
  "" | -*)
    printf 'usage: storeRepack.sh <repository> | storeRepack.sh --teams <directory>\n' >&2
    exit 2
    ;;
  *)
    repack "$1"
    ;;
esac
