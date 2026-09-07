#!/usr/bin/env bash
# Creates the store's bare repository on the host, and optionally a post-receive hook that
# mirrors every push to a private GitHub repository.
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
                           [--branch main] [--mirror git@github.com:owner/repo.git]

  --mirror  необязателен: если задан, ставится хук post-receive, который после каждого push
            делает `git push --mirror` в этот репозиторий. Ключ для GitHub должен уже лежать
            на сервере (см. docs/manuals/hostSetup.md).

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
repoPath="$1"
branch="$2"
mirror="$3"

command -v git >/dev/null 2>&1 || { echo "На сервере нет git" >&2; exit 1; }

if [ -d "$HOME/$repoPath" ]; then
  echo "1/3 Репозиторий уже есть: ~/$repoPath"
else
  echo "1/3 Создаю bare-репозиторий ~/$repoPath"
  mkdir -p "$HOME/$repoPath"
  git init --bare --quiet --initial-branch="$branch" "$HOME/$repoPath"
fi

# Settings that matter for a store of transcripts: bytes stay bytes, and a push of a fresh
# clone is not rejected for being unrelated.
git -C "$HOME/$repoPath" config core.autocrlf false
git -C "$HOME/$repoPath" config core.filemode false
git -C "$HOME/$repoPath" config gc.auto 0
git -C "$HOME/$repoPath" symbolic-ref HEAD "refs/heads/$branch"
echo "2/3 Настройки repo проставлены (autocrlf=false, filemode=false, gc.auto=0)"

hook="$HOME/$repoPath/hooks/post-receive"
if [ -n "$mirror" ]; then
  wanted="#!/usr/bin/env bash
# Mirrors every push to the private GitHub copy. Written by infra/hostBootstrap.sh.
set -euo pipefail
git push --mirror '$mirror' >/dev/null 2>&1 || echo 'vibememory: зеркало недоступно, пуш принят' >&2
"
  if [ -f "$hook" ] && [ "$(cat "$hook")" = "$wanted" ]; then
    echo "3/3 Хук зеркала уже стоит и совпадает"
  else
    printf '%s' "$wanted" > "$hook"
    chmod +x "$hook"
    echo "3/3 Хук зеркала записан"
  fi
else
  echo "3/3 Зеркало не настраивалось"
fi

echo
echo "Готово. Размер репозитория: $(du -sh "$HOME/$repoPath" | cut -f1)"
REMOTE

say ""
say "Дальше: ./infra/connectStore.sh — привязать локальный стор к серверу и запушить."
