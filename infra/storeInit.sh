#!/usr/bin/env bash
# Settings of a bare store repository on the host. The one place they live — for the owner's own
# store and for every team repository — so the two cannot drift apart.
#
#   storeInit.sh <path> [--adopted] [--branch main]
#
# Runs ON THE HOST as the repository's owner or root. Creates the repository when it is missing,
# never makes a commit, never deletes anything. Idempotent: a value or a file is written only when
# it differs, so a second run leaves the config file and the hook untouched.
#
# `--adopted` is the owner's store that existed before teams. It gets the common settings only:
# the `receive.*` guards and the `pre-receive` hook below would stop the owner's own engine, which
# is not bound by team rules and whose tick may push more than one limit's worth after a long time
# offline.
set -euo pipefail

# The server binary that decides every push to a team store: the path the host's layout gives it
# (crates/vibememory-mcp/src/layout.rs).
readonly SERVER_BIN=/srv/vibememory/bin/vibememory-mcp

repo=""
adopted=no
branch=main

while [ "$#" -gt 0 ]; do
  case "$1" in
    --adopted) adopted=yes; shift ;;
    --branch) branch="${2:?--branch needs a name}"; shift 2 ;;
    -*) printf 'storeInit.sh: unknown flag %s\n' "$1" >&2; exit 2 ;;
    *) repo="$1"; shift ;;
  esac
done
[ -n "$repo" ] || { printf 'usage: storeInit.sh <path> [--adopted] [--branch main]\n' >&2; exit 2; }

# `--git-dir` and never `-C`: git refuses `-C` into a repository owned by another user ("dubious
# ownership"), and on the host the owner's store and the team stores are written by different
# accounts. An explicit git dir is not subject to that check.
g() { git --git-dir="$repo" "$@"; }

# Writes a value only when it differs: `git config key value` rewrites the file even when the value
# is already there, and a rewrite on every run would make "the second run changed nothing"
# impossible to check.
set_config() {
  local key="$1" value="$2"
  [ "$(g config --get "$key" 2>/dev/null || true)" = "$value" ] || g config "$key" "$value"
}

if [ ! -d "$repo" ]; then
  git init --bare --quiet --initial-branch="$branch" --shared=group "$repo"
  printf 'storeInit.sh: created %s\n' "$repo"
fi

# Bytes stay bytes: transcripts and memory journals are compared byte for byte by the merge
# drivers, and a converted line ending would make every file look changed.
set_config core.autocrlf false
set_config core.filemode false
# Garbage collection stays out of the push path; packing is the nightly storeRepack.sh.
set_config gc.auto 0
# Incoming pushes stay packs with the sender's deltas. With git's default of 100 a small push is
# unpacked into loose files and every version of a growing transcript lands whole.
set_config transfer.unpackLimit 1
# No size threshold for delta search: it turned compression off for exactly the files the store is
# made of. Memory stays bounded by the two pack settings below.
if [ -n "$(g config --get-all core.bigFileThreshold 2>/dev/null || true)" ]; then
  g config --unset-all core.bigFileThreshold
fi
set_config pack.threads 1
set_config pack.windowMemory 64m
# New files are group-writable, so the memory server and the owner's account can both write here.
# It covers files created from now on; existing ones are brought over by hostBootstrap.sh.
set_config core.sharedRepository group

if [ "$(g symbolic-ref HEAD 2>/dev/null || true)" != "refs/heads/$branch" ]; then
  g symbolic-ref HEAD "refs/heads/$branch"
fi

if [ "$adopted" = no ]; then
  # A team repository has one branch and a history nobody rewrites: the memory journal is merged by
  # union, and a forced push or a deleted branch would drop other members' records.
  set_config receive.denyNonFastForwards true
  set_config receive.denyDeletes true
  # One push may not fill the disk that every team shares.
  set_config receive.maxInputSize 1g
  # Objects are checked as they arrive: a tree with a `..` or `.git` entry would break, or worse,
  # every other member's checkout of the store.
  set_config receive.fsckObjects true

  # Every push is decided against the access snapshot: whose key, which references, which paths,
  # how much room (docs/manuals/hostShellSpec.md). The hook only hands the push over.
  hook="$repo/hooks/pre-receive"
  wantedHook="#!/bin/sh
# Written by storeInit.sh: every push to this team store is decided by the server against the
# access snapshot.
exec $SERVER_BIN pre-receive
"
  mkdir -p "$repo/hooks"
  if ! printf '%s' "$wantedHook" | cmp -s - "$hook"; then
    printf '%s' "$wantedHook" > "$hook.new"
    chmod 755 "$hook.new"
    mv "$hook.new" "$hook"
    printf 'storeInit.sh: pre-receive hook written in %s\n' "$repo"
  fi
fi
