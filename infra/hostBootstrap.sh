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
git -C "$HOME/$repoPath" symbolic-ref HEAD "refs/heads/$branch"
echo "3/5 HEAD указывает на $branch"
echo "2/5 Настройки repo проставлены (autocrlf=false, filemode=false, gc.auto=0)"

# Weekly repacking. `gc.auto=0` above keeps garbage collection out of the push path — a tick
# must not wait for a repack — but a repository that is never packed grows without bound, and
# this disk is 10 GiB. Once a week, at night, out of anyone's way.
# `set -e` kills the script on the first non-zero status, and both `crontab -l` (no crontab yet)
# and `grep -q` (no match) return one legitimately. Hence `|| true` and an explicit `if`.
# `git gc --quiet` and nothing else: `--auto=0` is not a thing — `--auto` takes no value, and the
# weekly job would have failed silently every Sunday.
if command -v crontab >/dev/null 2>&1; then
  line="17 4 * * 0 git -C $HOME/$repoPath gc --quiet >/dev/null 2>&1"
  current="$(crontab -l 2>/dev/null || true)"
  if printf '%s\n' "$current" | grep -Fq "$repoPath gc"; then
    echo "4/5 Еженедельная упаковка уже в cron"
  else
    printf '%s\n%s\n' "$current" "$line" | grep -v '^$' | crontab -
    echo "4/5 Еженедельная упаковка добавлена в cron (вс 04:17)"
  fi
else
  echo "4/5 crontab не найден — упаковку придётся запускать вручную: git -C ~/$repoPath gc"
fi

hook="$HOME/$repoPath/hooks/post-receive"
if [ -n "$mirror" ]; then
  wanted="#!/usr/bin/env bash
# Mirrors every push to the private GitHub copy. Written by infra/hostBootstrap.sh.
set -euo pipefail
git push --mirror '$mirror' >/dev/null 2>&1 || echo 'vibememory: зеркало недоступно, пуш принят' >&2
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
