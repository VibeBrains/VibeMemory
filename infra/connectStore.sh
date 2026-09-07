#!/usr/bin/env bash
# Points this machine's store at the host and pushes it. Runs on the OWNER'S machine.
#
# Two things are deliberate. The remote is written into ~/.vibememory/config.json only after the
# push succeeds — a configuration naming a remote that does not work would make every tick try
# and fail. And nothing is force-pushed: the host is the shared history, and this machine is one
# of its writers, not its owner.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_PATH=vibememory/store.git
readonly DEFAULT_BRANCH=main

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
repoPath="${VIBEMEMORY_REPO_PATH:-$DEFAULT_PATH}"
branch="${VIBEMEMORY_BRANCH:-$DEFAULT_BRANCH}"
engineDir="${VIBEMEMORY_DIR:-$HOME/.vibememory}"

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --alias)  sshAlias="${2:-}"; shift 2 ;;
    --path)   repoPath="${2:-}"; shift 2 ;;
    --branch) branch="${2:-}"; shift 2 ;;
    -h | --help)
      cat <<TEXT
Привязать локальный стор к серверу и запушить.

  ./infra/connectStore.sh [--alias vibememory] [--path vibememory/store.git] [--branch main]

Переменные окружения: VIBEMEMORY_SSH_ALIAS, VIBEMEMORY_REPO_PATH, VIBEMEMORY_BRANCH,
VIBEMEMORY_DIR.
TEXT
      exit 0 ;;
    *) fail "Неизвестный аргумент: $1" ;;
  esac
done

store="$engineDir/store"
config="$engineDir/config.json"
[ -d "$store/.git" ] || fail "Нет стора в $store — сначала vibememory install"
[ -f "$config" ] || fail "Нет конфига $config"
command -v python3 >/dev/null 2>&1 || fail "Не найден python3 (нужен для правки config.json)"

remote="${sshAlias}:${repoPath}"
say "Стор: $store"
say "Remote: $remote (ветка $branch)"
say ""

current="$(git -C "$store" remote get-url origin 2>/dev/null || true)"
if [ -z "$current" ]; then
  say "1/3 Добавляю origin"
  git -C "$store" remote add origin "$remote"
elif [ "$current" = "$remote" ]; then
  say "1/3 origin уже указывает туда"
else
  fail "origin уже задан и указывает на «$current». Сначала разберитесь вручную: перенацеливать чужой remote скрипт не станет."
fi

# The first push of a store is hundreds of megabytes over a home uplink, and git cannot resume:
# one dropped connection throws away everything transferred. Measured 2026-09-07 — 17 minutes,
# then a broken pipe and an empty repository. So history goes in chunks: every push after the
# first sends only what the previous one did not, and a failure costs one chunk, not the lot.
readonly CHUNK=300

say "2/3 Пушу ветку $branch (без force — история сервера главнее)"
remoteHas="$(git -C "$store" ls-remote --heads origin "$branch" 2>/dev/null | wc -l | tr -d ' ')"
if [ "$remoteHas" = "0" ]; then
  total="$(git -C "$store" rev-list --count "$branch")"
  say "    первый push: $total коммит(ов), иду порциями по $CHUNK"
  sent=0
  for sha in $(git -C "$store" rev-list --reverse "$branch" | awk -v n="$CHUNK" 'NR % n == 0'); do
    sent=$((sent + CHUNK))
    say "    … до коммита $sent из $total"
    git -C "$store" push --quiet origin "${sha}:refs/heads/${branch}" ||
      fail "порция не ушла. Запустите скрипт снова — он продолжит с этого места."
  done
fi
git -C "$store" push -u origin "$branch"

# 3. The configuration last: a remote in config.json that does not work would make every tick
# spend its two minutes failing.
say "3/3 Записываю remote в config.json"
python3 - "$config" "$remote" <<'PY'
import io, json, sys
path, remote = sys.argv[1], sys.argv[2]
with io.open(path, encoding="utf-8") as handle:
    config = json.load(handle)
config["remote"] = remote
with io.open(path, "w", encoding="utf-8") as handle:
    json.dump(config, handle, ensure_ascii=False, indent=2)
    handle.write("\n")
PY

say ""
say "Готово. Проверка: vibememory tick"
