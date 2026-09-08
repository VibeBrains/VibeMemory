#!/usr/bin/env bash
# Points the store's host at the OWNER'S OWN private GitHub repository as a backup mirror.
#
# Every user of the engine has a different backup repository, so nothing here is baked in: the
# repository is an argument, and the deploy key is made on the host and never leaves it.
#
# Runs on the owner's machine over the key seeded by seedKey.sh. Idempotent: an existing key,
# ssh alias and known_hosts entry are left alone, and a deploy key already registered on GitHub
# is recognised instead of added twice.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_PATH=vibememory/store.git
# The ssh alias made on the host. Deliberately not `github.com`: a global entry would hijack
# every other GitHub access from that box.
readonly HOST_ALIAS=githubMirror
readonly HOST_KEY='~/.ssh/githubMirror'

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
repoPath="${VIBEMEMORY_REPO_PATH:-$DEFAULT_PATH}"
repo="${VIBEMEMORY_MIRROR_REPO:-}"
seed=yes

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Настроить зеркало стора в свой приватный репозиторий GitHub.

  ./infra/mirrorSetup.sh --repo owner/name [--alias vibememory]
                         [--path vibememory/store.git] [--no-seed]

  --repo     ваш ПРИВАТНЫЙ репозиторий на GitHub, куда хост будет складывать бэкап
             (создайте заранее пустым: gh repo create owner/name --private).
  --no-seed  не делать первый залив: хук будет пушить только новые коммиты.

Что делает: заводит на хосте отдельный ключ $HOST_KEY и ssh-алиас $HOST_ALIAS, добавляет
github.com в known_hosts, регистрирует ключ как deploy key с правом записи (если на этой
машине есть gh с доступом к репозиторию — иначе печатает ключ и ссылку), ставит хук зеркала
через hostBootstrap.sh и запускает первый залив на хосте фоном.

Переменные окружения: VIBEMEMORY_SSH_ALIAS, VIBEMEMORY_REPO_PATH, VIBEMEMORY_MIRROR_REPO.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo)  repo="${2:-}"; shift 2 ;;
    --alias) sshAlias="${2:-}"; shift 2 ;;
    --path)  repoPath="${2:-}"; shift 2 ;;
    --no-seed) seed=no; shift ;;
    -h | --help) usage; exit 0 ;;
    *) fail "Неизвестный аргумент: $1 (--help покажет список)" ;;
  esac
done

[ -n "$repo" ] || { usage; fail "Не задан --repo owner/name"; }
case "$repo" in
  */*/*) fail "Ожидается owner/name, а не URL: $repo" ;;
  */*) : ;;
  *) fail "Ожидается owner/name: $repo" ;;
esac

ssh -o BatchMode=yes "$sshAlias" 'echo ok' >/dev/null 2>&1 ||
  fail "Сервер не пускает по ключу. Сначала ./infra/seedKey.sh"

here="$(cd "$(dirname "$0")" && pwd)"
mirrorUrl="$HOST_ALIAS:$repo.git"

say "Сервер: $sshAlias, репозиторий стора: ~/$repoPath"
say "Зеркало: $repo (приватный GitHub)"
say ""

# 1. Key, ssh alias and known_hosts — all on the host, in one session.
pubkey="$(ssh -o BatchMode=yes "$sshAlias" 'bash -s' <<'REMOTE'
set -euo pipefail
key="$HOME/.ssh/githubMirror"
mkdir -p "$HOME/.ssh"; chmod 700 "$HOME/.ssh"
[ -f "$key" ] || ssh-keygen -t ed25519 -N '' -C 'vibememory-mirror@host' -f "$key" >/dev/null
ssh-keyscan -t ed25519 github.com 2>/dev/null >> "$HOME/.ssh/known_hosts"
sort -u "$HOME/.ssh/known_hosts" -o "$HOME/.ssh/known_hosts"
if ! grep -q '^Host githubMirror$' "$HOME/.ssh/config" 2>/dev/null; then
  cat >> "$HOME/.ssh/config" <<CFG
Host githubMirror
  HostName github.com
  User git
  IdentityFile ~/.ssh/githubMirror
  IdentitiesOnly yes
CFG
fi
chmod 600 "$HOME/.ssh/config"
cat "$key.pub"
REMOTE
)"
[ -n "$pubkey" ] || fail "Не удалось получить публичный ключ с сервера"
say "1/4 Ключ и ssh-алиас $HOST_ALIAS на месте"

# 2. Deploy key on GitHub. Registering it needs write access to the repository; when this
# machine cannot do that, the owner does it by hand — the script says exactly where.
fingerprint="$(printf '%s' "$pubkey" | awk '{print $2}')"
if command -v gh >/dev/null 2>&1 && gh api "repos/$repo" >/dev/null 2>&1; then
  if gh api "repos/$repo/keys" --jq '.[].key' 2>/dev/null | grep -Fq "$fingerprint"; then
    say "2/4 Deploy key уже зарегистрирован"
  else
    gh api "repos/$repo/keys" -f title="vibememory host ($sshAlias)" -f key="$pubkey" \
      -F read_only=false >/dev/null
    say "2/4 Deploy key зарегистрирован с правом записи"
  fi
else
  say "2/4 gh недоступен или нет прав на $repo — добавьте ключ сами:"
  say "     https://github.com/$repo/settings/keys/new (Allow write access — обязательно)"
  say ""
  say "$pubkey"
  say ""
  read -r -p "     Нажмите Enter, когда ключ добавлен: " _
fi

ssh -o BatchMode=yes "$sshAlias" "ssh -o BatchMode=yes -T $HOST_ALIAS 2>&1 | grep -q '$repo'" ||
  fail "Хост не аутентифицируется в $repo по deploy-ключу. Проверьте, что ключ добавлен именно в этот репозиторий."
say "3/4 Хост аутентифицируется в $repo по deploy-ключу"

# 3. The hook itself.
"$here/hostBootstrap.sh" --alias "$sshAlias" --path "$repoPath" --mirror "$mirrorUrl" >/dev/null
say "4/4 Хук зеркала поставлен: post-receive → $repo"

# 4. First upload, on the host and in the background. A first push of a store is hundreds of
# megabytes; inside post-receive it would hold up the tick that triggered it for minutes.
if [ "$seed" = yes ]; then
  ssh -o BatchMode=yes "$sshAlias" \
    "cd ~/$repoPath && nohup git push --mirror '$mirrorUrl' > ~/mirror-seed.log 2>&1 & echo" >/dev/null
  say ""
  say "Первый залив запущен на сервере фоном. Ход: ssh $sshAlias 'tail -f ~/mirror-seed.log'"
fi

say ""
say "Проверка: vibememory doctor покажет строку зеркала — головы хоста и GitHub должны совпасть."
