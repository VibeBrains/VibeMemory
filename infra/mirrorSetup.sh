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
# The ssh alias made on the host. Deliberately not the provider's own hostname: a global entry
# would hijack every other access to that provider from this box. Named after what it is for —
# the store's mirror — and not after a provider, because it works with any of them.
readonly HOST_ALIAS=storeMirror
readonly HOST_KEY='~/.ssh/storeMirror'
# The first release called both `githubMirror`. An existing key is reused rather than replaced:
# it is the one already registered with the provider, and a new one would silently lose access.
readonly LEGACY_ALIAS=githubMirror
# Where the host logs the first upload. One name, used by the command and by the hint that
# tells the owner how to watch it.
readonly SEED_LOG='~/vibememory-mirror-seed.log'

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
repoPath="${VIBEMEMORY_REPO_PATH:-$DEFAULT_PATH}"
repo="${VIBEMEMORY_MIRROR_REPO:-}"
gitHost="${VIBEMEMORY_MIRROR_HOST:-github.com}"
seed=yes

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Настроить зеркало стора в свой приватный репозиторий GitHub.

  ./infra/mirrorSetup.sh --repo owner/name [--alias vibememory]
                         [--path vibememory/store.git] [--no-seed]

  --repo     ваш ПРИВАТНЫЙ репозиторий, куда хост будет складывать бэкап — пустым
             (GitHub: gh repo create owner/name --private).
  --host     git-хостинг, если это не github.com: gitlab.com, codeberg.org, свой сервер.
  --no-seed  не делать первый залив: хук будет пушить только новые коммиты.

Что делает: заводит на хосте отдельный ключ $HOST_KEY и ssh-алиас $HOST_ALIAS, добавляет
хостинг в known_hosts, регистрирует ключ как deploy key с правом записи (на github.com — сам
через gh, если он есть и имеет доступ; иначе печатает ключ и ссылку), ставит хук зеркала
через hostBootstrap.sh и запускает первый залив на хосте фоном.

Переменные окружения: VIBEMEMORY_SSH_ALIAS, VIBEMEMORY_REPO_PATH, VIBEMEMORY_MIRROR_REPO.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo)  repo="${2:-}"; shift 2 ;;
    --host)  gitHost="${2:-}"; shift 2 ;;
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

[ -n "$gitHost" ] || fail "Пустой --host"

ssh -o BatchMode=yes "$sshAlias" 'echo ok' >/dev/null 2>&1 ||
  fail "Сервер не пускает по ключу. Сначала ./infra/seedKey.sh"

here="$(cd "$(dirname "$0")" && pwd)"
mirrorUrl="$HOST_ALIAS:$repo.git"

say "Сервер: $sshAlias, репозиторий стора: ~/$repoPath"
say "Зеркало: $repo на $gitHost (приватный репозиторий)"
say ""

# 1. Key, ssh alias and known_hosts — all on the host, in one session.
pubkey="$(ssh -o BatchMode=yes "$sshAlias" 'bash -s' -- "$HOST_ALIAS" "$LEGACY_ALIAS" "$gitHost" <<'REMOTE'
set -euo pipefail
alias="$1"; legacy="$2"; gitHost="$3"
mkdir -p "$HOME/.ssh"; chmod 700 "$HOME/.ssh"
key="$HOME/.ssh/$alias"
# A host set up by the first release has the key under the old name. It is already registered
# with the provider, so it is moved, not regenerated — a fresh key would authenticate as nobody.
if [ ! -f "$key" ] && [ -f "$HOME/.ssh/$legacy" ]; then
  mv "$HOME/.ssh/$legacy" "$key"
  mv "$HOME/.ssh/$legacy.pub" "$key.pub"
  # The old Host block now points at a file that is gone. Left in place it would be a trap for
  # whoever reads this config next, so it goes with the key it described.
  if [ -f "$HOME/.ssh/config" ]; then
    awk -v skip="Host $legacy" '
      $0 == skip { dropping = 1; next }
      /^Host / { dropping = 0 }
      !dropping { print }
    ' "$HOME/.ssh/config" > "$HOME/.ssh/config.new"
    mv "$HOME/.ssh/config.new" "$HOME/.ssh/config"
    chmod 600 "$HOME/.ssh/config"
  fi
fi
[ -f "$key" ] || ssh-keygen -t ed25519 -N '' -C 'vibememory-mirror@host' -f "$key" >/dev/null
ssh-keyscan -t ed25519 "$gitHost" 2>/dev/null >> "$HOME/.ssh/known_hosts"
sort -u "$HOME/.ssh/known_hosts" -o "$HOME/.ssh/known_hosts"
if ! grep -q "^Host $alias\$" "$HOME/.ssh/config" 2>/dev/null; then
  cat >> "$HOME/.ssh/config" <<CFG

Host $alias
  HostName $gitHost
  User git
  IdentityFile ~/.ssh/$alias
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
if [ "$gitHost" = github.com ] && command -v gh >/dev/null 2>&1 && gh api "repos/$repo" >/dev/null 2>&1; then
  if gh api "repos/$repo/keys" --jq '.[].key' 2>/dev/null | grep -Fq "$fingerprint"; then
    say "2/4 Deploy key уже зарегистрирован"
  else
    gh api "repos/$repo/keys" -f title="vibememory host ($sshAlias)" -f key="$pubkey" \
      -F read_only=false >/dev/null
    say "2/4 Deploy key зарегистрирован с правом записи"
  fi
else
  say "2/4 Добавьте ключ как deploy key С ПРАВОМ ЗАПИСИ:"
  say "     https://$gitHost/$repo/settings/keys (GitLab: can_push, Gitea: снять Read only)"
  say ""
  say "$pubkey"
  say ""
  # In a pipeline, a cron job or another script there is nobody to press Enter, and a script that
  # blocks forever is worse than one that says what is missing and stops.
  if [ -t 0 ]; then
    read -r -p "     Нажмите Enter, когда ключ добавлен: " _
  else
    fail "Ключ не зарегистрирован, а спросить некого (не интерактивный запуск). Добавьте ключ и запустите снова."
  fi
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
    "cd ~/$repoPath && nohup git push --mirror '$mirrorUrl' > $SEED_LOG 2>&1 & echo" >/dev/null
  say ""
  say "Первый залив запущен на сервере фоном. Ход: ssh $sshAlias 'tail -f $SEED_LOG'"
fi

say ""
say "Проверка: vibememory doctor покажет строку зеркала — головы хоста и GitHub должны совпасть."
