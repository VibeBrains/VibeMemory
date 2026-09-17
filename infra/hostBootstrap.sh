#!/usr/bin/env bash
# Creates the store's bare repository on the host, and optionally a post-receive hook that
# mirrors every push to a private repository at any git host.
#
# Runs on the OWNER'S machine over the key seeded by seedKey.sh; the server side is a single
# ssh session with a here-document, so nothing has to be copied there first.
#
# Idempotent: an existing repository is left alone, the hook is rewritten only when its text
# differs. Nothing is ever deleted.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_PATH=vibememory/store.git
readonly DEFAULT_BRANCH=main

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
repoPath="${VIBEMEMORY_REPO_PATH:-$DEFAULT_PATH}"
branch="${VIBEMEMORY_BRANCH:-$DEFAULT_BRANCH}"
mirror="${VIBEMEMORY_MIRROR:-}"

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Завести bare-репозиторий стора на сервере.

  ./infra/hostBootstrap.sh [--alias vibememory] [--path vibememory/store.git]
                           [--branch main] [--mirror storeMirror:owner/repo.git]

  --mirror  необязателен: если задан, ставится хук post-receive, который после каждого push
            делает `git push --mirror` в этот репозиторий. Ключ и ssh-алиас на сервере заводит
            ./infra/mirrorSetup.sh — вызывать этот скрипт с --mirror руками обычно не нужно.

Переменные окружения: VIBEMEMORY_SSH_ALIAS, VIBEMEMORY_REPO_PATH, VIBEMEMORY_BRANCH,
VIBEMEMORY_MIRROR.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --alias)  sshAlias="${2:-}"; shift 2 ;;
    --path)   repoPath="${2:-}"; shift 2 ;;
    --branch) branch="${2:-}"; shift 2 ;;
    --mirror) mirror="${2:-}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) fail "Неизвестный аргумент: $1 (--help покажет список)" ;;
  esac
done

[ -n "$sshAlias" ] || fail "Пустой алиас"
[ -n "$repoPath" ] || fail "Пустой путь репозитория"
[ -n "$branch" ] || fail "Пустая ветка"

ssh -o BatchMode=yes "$sshAlias" 'echo ok' >/dev/null 2>&1 ||
  fail "Сервер не пускает по ключу. Сначала ./infra/seedKey.sh"

say "Сервер: $sshAlias, репозиторий: ~/$repoPath, ветка: $branch"
[ -n "$mirror" ] && say "Зеркало: $mirror" || say "Зеркало: не настраивается (--mirror не задан)"
say ""

# The whole server side in one session. `bash -s` reads the script from stdin, so the quoting
# rules are the ordinary ones and nothing needs escaping twice.
ssh -o BatchMode=yes "$sshAlias" 'bash -s' -- "$repoPath" "$branch" "$mirror" <<'REMOTE'
set -euo pipefail
# git is not on a fresh Debian by default; the server side says so plainly rather than failing
# three commands later.
# `${3:-}` and not `$3`: ssh glues the arguments into one command line, so an empty last
# argument disappears entirely, and `set -u` then kills the script on the server. Found on the
# first real run.
repoPath="${1:?путь репозитория не передан}"
branch="${2:?ветка не передана}"
mirror="${3:-}"

command -v git >/dev/null 2>&1 || {
  echo "На сервере нет git. Установите: sudo apt-get install -y git" >&2
  exit 1
}

if [ -d "$HOME/$repoPath" ]; then
  echo "1/5 Репозиторий уже есть: ~/$repoPath"
else
  echo "1/5 Создаю bare-репозиторий ~/$repoPath"
  mkdir -p "$HOME/$repoPath"
  git init --bare --quiet --initial-branch="$branch" "$HOME/$repoPath"
fi

# Settings that matter for a store of transcripts: bytes stay bytes, and a push of a fresh
# clone is not rejected for being unrelated.
git -C "$HOME/$repoPath" config core.autocrlf false
git -C "$HOME/$repoPath" config core.filemode false
git -C "$HOME/$repoPath" config gc.auto 0
# Incoming pushes stay packs. With git's default of 100 a push of fewer objects is unpacked into
# loose files, and every version of a growing transcript then lands whole, without deltas: on
# 2026-09-12 that was 1.6 GiB of loose objects in a day, and the 10 GiB disk filled up.
git -C "$HOME/$repoPath" config transfer.unpackLimit 1
# No size threshold for delta search. `core.bigFileThreshold=8m` was set once "to save memory" and
# turned delta compression off for exactly the files the store is made of: a push brings the big
# versions of a transcript whole, and the host could never compress them. On 2026-09-17 238 such
# objects held 2.41 GiB of a 2.79 GiB store — the Mac held the same history in 1.08 GiB — and the
# nightly repack filled the disk a second time. Memory stays bounded by pack.windowMemory and
# pack.threads below. `--unset-all` exits 5 when nothing is set, which is the normal case.
git -C "$HOME/$repoPath" config --unset-all core.bigFileThreshold || true
git -C "$HOME/$repoPath" config pack.threads 1
git -C "$HOME/$repoPath" config pack.windowMemory 64m
git -C "$HOME/$repoPath" symbolic-ref HEAD "refs/heads/$branch"
echo "3/5 HEAD указывает на $branch"
echo "2/5 Настройки repo проставлены (autocrlf=false, filemode=false, gc.auto=0, unpackLimit=1, без bigFileThreshold)"

# The store holds transcripts of every session: prompts, code, file contents. On a box with one
# account world-readable changes nothing today, but the day a second account appears it changes
# everything — and nobody re-checks permissions on that day.
chmod 700 "$HOME/$repoPath" "$(dirname "$HOME/$repoPath")"

# Nightly repacking, incremental. `gc.auto=0` above keeps garbage collection out of the push path —
# a tick must not wait for a repack — but every push lands as its own pack (`unpackLimit=1`), and
# hundreds of small packs slow every read.
#
# Not `git gc`. It rewrites the whole store into one new pack, so it needs as much free space as
# the store occupies: on 2026-09-16 it did that once, and on 2026-09-17 it died half-way at 2.23 GiB
# with the disk full, and pushes stopped. `repack --geometric=2` merges the small packs and leaves
# the big one alone while the small ones stay small.
#
# The script refuses to start without room for the worst case — every pack rewritten plus a
# reserve — and says so in the journal: `journalctl -t vibememory-repack`. The old line sent its
# output to /dev/null, which is how a failed repack stayed invisible for two nights.
#
# `set -e` kills the script on the first non-zero status, and both `crontab -l` (no crontab yet)
# and `grep -q` (no match) return one legitimately. Hence `|| true` and an explicit `if`.
repack="$(dirname "$HOME/$repoPath")/bin/storeRepack.sh"
mkdir -p "$(dirname "$repack")"
wantedRepack='#!/usr/bin/env bash
# Nightly incremental repack of the store. Written by infra/hostBootstrap.sh.
set -uo pipefail
repo="${1:?repository path}"
readonly RESERVE_BYTES=$((1024 * 1024 * 1024))
readonly TAG=vibememory-repack
say() { logger -t "$TAG" -- "$*"; }

packs="$repo/objects/pack"
[ -d "$packs" ] || { say "no pack directory in $repo"; exit 1; }
# printf "%.0f", not print: awk prints large sums in exponent form, and under a Russian locale
# with a decimal comma ("3,63136e+09"), which bash arithmetic rejects. Found on the first live run.
packBytes=$(find "$packs" -maxdepth 1 -name "pack-*.pack" -printf "%s\n" | LC_ALL=C awk "{s+=\$1} END {printf \"%.0f\", s}")
freeBytes=$(df -B1 --output=avail "$packs" | tail -1 | tr -d " ")
needBytes=$((packBytes + RESERVE_BYTES))
if [ "$freeBytes" -lt "$needBytes" ]; then
  say "skipped: $freeBytes bytes free, $needBytes needed (packs $packBytes + reserve $RESERVE_BYTES)"
  exit 1
fi
leftover=$(find "$packs" -maxdepth 1 -name "tmp_pack_*" | wc -l)
[ "$leftover" -eq 0 ] || say "warning: $leftover tmp_pack file(s) left by an earlier run"
if out=$(git -C "$repo" repack -d --geometric=2 --quiet 2>&1); then
  say "done: $(git -C "$repo" count-objects -v | tr "\n" " ")"
else
  say "failed: $out"
  exit 1
fi
'
if [ -f "$repack" ] && [ "$(cat "$repack")" = "$wantedRepack" ]; then
  echo "4/5 Скрипт упаковки уже стоит и совпадает"
else
  printf '%s' "$wantedRepack" > "$repack"
  chmod 755 "$repack"
  echo "4/5 Скрипт упаковки записан: $repack"
fi

if command -v crontab >/dev/null 2>&1; then
  line="17 4 * * * $repack $HOME/$repoPath"
  current="$(crontab -l 2>/dev/null || true)"
  if printf '%s\n' "$current" | grep -Fxq "$line"; then
    echo "4/5 Ночная упаковка уже в cron"
  else
    # Earlier lines are replaced, not left beside the new one: the weekly and nightly `gc` of
    # previous bootstraps, and a repack line pointing elsewhere.
    printf '%s\n%s\n' "$(printf '%s\n' "$current" | grep -Fv "$repoPath gc" | grep -Fv "storeRepack.sh" || true)" "$line" | grep -v '^$' | crontab -
    echo "4/5 Ночная упаковка поставлена в cron (04:17, частичная, журнал: journalctl -t vibememory-repack)"
  fi
else
  echo "4/5 crontab не найден — упаковку придётся запускать вручную: $repack ~/$repoPath"
fi

hook="$HOME/$repoPath/hooks/post-receive"
if [ -n "$mirror" ]; then
  # Detached on purpose. A post-receive hook runs while the client still holds the push open, so
  # a synchronous mirror push makes every machine wait for the provider — and a slow or wedged
  # provider would stall the tick that started it. The mirror is a backup: it may lag by seconds.
  # `flock` keeps a burst of pushes from starting a pile of uploads of the same repository; a
  # skipped run costs nothing, because the next push mirrors the same refs anyway.
  wanted="#!/usr/bin/env bash
# Mirrors every push to the private copy, in the background. Written by infra/hostBootstrap.sh.
set -euo pipefail
log=\"\$HOME/vibememory-mirror.log\"
(
  if command -v flock >/dev/null 2>&1; then
    flock -n 9 || exit 0
  fi
  git push --mirror '$mirror' >>\"\$log\" 2>&1 ||
    echo \"vibememory: зеркало недоступно \$(date -u +%FT%TZ)\" >>\"\$log\"
) 9>\"\$HOME/.vibememory-mirror.lock\" &
disown 2>/dev/null || true
"
  if [ -f "$hook" ] && [ "$(cat "$hook")" = "$wanted" ]; then
    echo "5/5 Хук зеркала уже стоит и совпадает"
  else
    printf '%s' "$wanted" > "$hook"
    chmod +x "$hook"
    echo "5/5 Хук зеркала записан"
  fi
else
  echo "5/5 Зеркало не настраивалось"
fi

echo
echo "Готово. Размер репозитория: $(du -sh "$HOME/$repoPath" | cut -f1)"
REMOTE

say ""
say "Дальше: ./infra/connectStore.sh — привязать локальный стор к серверу и запушить."
