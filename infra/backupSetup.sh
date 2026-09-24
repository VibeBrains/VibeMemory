#!/usr/bin/env bash
# Sets up the nightly encrypted backup of the memory teams' stores: the owner's age key on this
# machine, a private repository for the backup with a deploy key of the host, and the host's timer.
#
# Runs on the OWNER'S machine over the key seeded by seedKey.sh, after hostMcp.sh. The age key is
# made here and never leaves this machine: the host gets its public half only, so a host that is
# taken cannot read its own backups. Keep a copy of the key where you keep your passwords — without
# it no backup can be opened.
#
# Idempotent: an existing age key, repository, deploy key, ssh alias and unit are reused; files on
# the host are rewritten only when their text differs.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_REPO=VibeBrains/VibeMemoryBackup
readonly KEY_FILE="$HOME/.vibememory/keys/backup.age"
# The ssh alias of the backup on the host, named after what it is for, like storeMirror.
readonly HOST_ALIAS=memoryBackup
readonly BACKUP_DIR=/srv/vibememory/backup
readonly BIN_REMOTE=/srv/vibememory/bin
# An hour before the team stores are repacked (hostMcp.sh): the backup reads them first.
readonly BACKUP_AT='*-*-* 03:47:00'

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
repo="${VIBEMEMORY_BACKUP_REPO:-$DEFAULT_REPO}"
ghUser=""

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Настроить ночной зашифрованный бэкап сторов memory-команд.

  ./infra/backupSetup.sh [--repo $DEFAULT_REPO] [--gh-user <учётка gh>] [--alias vibememory]

  --repo     приватный репозиторий GitHub для бэкапа; создаётся, если его нет.
  --gh-user  учётка gh, от имени которой создаётся репозиторий и ставится deploy key,
             если это не активная учётка (gh auth status покажет обе).

Ключ age создаётся на этой машине: $KEY_FILE.
На хост уходит только его открытая половина.
Сохраните копию ключа там, где храните пароли: без него бэкап не открыть.

Переменные окружения: VIBEMEMORY_SSH_ALIAS, VIBEMEMORY_BACKUP_REPO.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo) repo="${2:-}"; shift 2 ;;
    --gh-user) ghUser="${2:-}"; shift 2 ;;
    --alias) sshAlias="${2:-}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) fail "Неизвестный аргумент: $1 (--help покажет список)" ;;
  esac
done

case "$repo" in
  */*/*) fail "Ожидается owner/name, а не URL: $repo" ;;
  */*) : ;;
  *) fail "Ожидается owner/name: $repo" ;;
esac
command -v age-keygen >/dev/null || fail "нет age-keygen (brew install age)"
command -v gh >/dev/null || fail "нет gh"
ssh -o BatchMode=yes "$sshAlias" 'echo ok' >/dev/null 2>&1 ||
  fail "Сервер не пускает по ключу. Сначала ./infra/seedKey.sh"

here="$(cd "$(dirname "$0")" && pwd)"

# gh as the account that owns the repository, without switching the active one: its token goes to
# this script's gh calls through the environment and is never printed.
asOwner() {
  if [ -n "$ghUser" ]; then
    GH_TOKEN="$(gh auth token --user "$ghUser")" gh "$@"
  else
    gh "$@"
  fi
}

# 1. The age key: made once, here, readable by this account alone.
if [ ! -f "$KEY_FILE" ]; then
  mkdir -p "$(dirname "$KEY_FILE")"
  chmod 700 "$(dirname "$KEY_FILE")"
  (umask 077 && age-keygen -o "$KEY_FILE" 2>/dev/null)
  say "1/6 Ключ age создан: $KEY_FILE — сохраните копию там, где храните пароли"
else
  say "1/6 Ключ age уже есть: $KEY_FILE"
fi
recipient="$(age-keygen -y "$KEY_FILE")"
case "$recipient" in age1*) : ;; *) fail "не удалось прочитать открытый ключ из $KEY_FILE" ;; esac

# 2. The private repository.
if asOwner repo view "$repo" >/dev/null 2>&1; then
  say "2/6 Репозиторий $repo уже есть"
else
  asOwner repo create "$repo" --private \
    --description "VibeMemory: nightly encrypted backup of the memory teams' stores" >/dev/null
  say "2/6 Репозиторий $repo создан, приватный"
fi
[ "$(asOwner repo view "$repo" --json visibility --jq .visibility)" = PRIVATE ] ||
  fail "$repo не приватный — бэкап туда не пойдёт"

# 3. age, the recipient and the backup script on the host; the deploy key, ssh alias and GitHub's
# host keys for vmgit. GitHub's keys come from its API over this machine's authenticated channel,
# not from a keyscan on the host, which would trust whoever answered first.
githubKeys="$(asOwner api meta --jq '.ssh_keys[]' | sed 's/^/github.com /')"
[ -n "$githubKeys" ] || fail "GitHub не отдал свои ключи хоста"
pubkey="$(ssh -o BatchMode=yes "$sshAlias" "bash -s -- $(printf '%q ' "$HOST_ALIAS" "$BACKUP_DIR" "$recipient" "$githubKeys")" <<'REMOTE'
set -euo pipefail
alias="$1"; backupDir="$2"; recipient="$3"; githubKeys="$4"
command -v age >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq age >/dev/null
[ -d "$backupDir" ] || sudo install -d -o root -g root -m 755 "$backupDir"
if [ "$(sudo cat "$backupDir/recipient.txt" 2>/dev/null || true)" != "$recipient" ]; then
  printf '%s\n' "$recipient" | sudo tee "$backupDir/recipient.txt" >/dev/null
  sudo chmod 644 "$backupDir/recipient.txt"
fi
sudo -u vmgit -H bash -s -- "$alias" "$githubKeys" <<'VMGIT'
set -euo pipefail
alias="$1"; githubKeys="$2"
cd "$HOME"
mkdir -p .ssh
chmod 700 .ssh
key=".ssh/$alias"
[ -f "$key" ] || ssh-keygen -t ed25519 -N '' -C 'vibememory-backup@host' -f "$key" >/dev/null
printf '%s\n' "$githubKeys" > .ssh/known_hosts.new
if [ -f .ssh/known_hosts ]; then
  grep -v '^github\.com ' .ssh/known_hosts >> .ssh/known_hosts.new || true
fi
sort -u .ssh/known_hosts.new -o .ssh/known_hosts.new
chmod 600 .ssh/known_hosts.new
mv -f .ssh/known_hosts.new .ssh/known_hosts
if ! grep -q "^Host $alias\$" .ssh/config 2>/dev/null; then
  printf '\nHost %s\n  HostName github.com\n  User git\n  IdentityFile ~/.ssh/%s\n  IdentitiesOnly yes\n  StrictHostKeyChecking yes\n  BatchMode yes\n' \
    "$alias" "$alias" >> .ssh/config
fi
chmod 600 .ssh/config
cat "$key.pub"
VMGIT
REMOTE
)"
[ -n "$pubkey" ] || fail "Не удалось получить публичный ключ vmgit с сервера"
say "3/6 На хосте: age, открытый ключ получателя, deploy key и алиас $HOST_ALIAS у vmgit"

# 4. The deploy key, with write access: the host pushes the night's commit.
material="$(printf '%s' "$pubkey" | awk '{print $2}')"
if asOwner api "repos/$repo/keys" --jq '.[].key' | grep -Fq "$material"; then
  say "4/6 Deploy key уже зарегистрирован"
else
  asOwner api "repos/$repo/keys" -f title="vibememory host backup ($sshAlias)" -f key="$pubkey" \
    -F read_only=false >/dev/null
  say "4/6 Deploy key зарегистрирован с правом записи"
fi

# 5. The script and its timer, as vmgit: it reads the team stores and the snapshot, and writes
# only backup.json beside the snapshot.
ssh -o BatchMode=yes "$sshAlias" "tmp=\$(mktemp); cat > \"\$tmp\";
  if sudo cmp -s \"\$tmp\" $BIN_REMOTE/hostBackup.sh; then :;
  else sudo install -o root -g root -m 755 \"\$tmp\" $BIN_REMOTE/hostBackup.sh; fi;
  rm -f \"\$tmp\"" < "$here/hostBackup.sh"
ssh -o BatchMode=yes "$sshAlias" "bash -s -- $(printf '%q ' "$BIN_REMOTE/hostBackup.sh" "$HOST_ALIAS:$repo.git" "$BACKUP_AT")" <<'REMOTE'
set -euo pipefail
script="$1"; remote="$2"; at="$3"
reload=0
putUnit() {
  local file="/etc/systemd/system/$1" text="$2"
  if [ "$(sudo cat "$file" 2>/dev/null || true)" != "$text" ]; then
    printf '%s\n' "$text" | sudo tee "$file" >/dev/null
    reload=1
  fi
}
putUnit vibememory-backup.service "[Unit]
Description=VibeMemory: encrypted backup of the memory teams' stores
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
User=vmgit
UMask=0077
ExecStart=$script $remote
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/srv/vibememory/access"
putUnit vibememory-backup.timer "[Unit]
Description=VibeMemory: back up the memory teams' stores every night

[Timer]
OnCalendar=$at
Persistent=true

[Install]
WantedBy=timers.target"
[ "$reload" = 0 ] || sudo systemctl daemon-reload
sudo systemctl enable --now --quiet vibememory-backup.timer
echo "5/6 Юнит и таймер бэкапа под vmgit: $(systemctl show -p NextElapseUSecRealtime --value vibememory-backup.timer)"
REMOTE

# 6. The first backup now, and proof that it can be opened here.
ssh -o BatchMode=yes "$sshAlias" 'sudo systemctl start vibememory-backup.service && sudo cat /srv/vibememory/access/backup.json' ||
  fail "бэкап не прошёл — journalctl -t vibememory-backup на хосте"
check="$(mktemp -d)"
trap 'rm -rf "$check"' EXIT
asOwner repo clone "$repo" "$check/backup" -- --quiet --depth 1 >/dev/null 2>&1 ||
  fail "не удалось склонировать $repo для проверки"
opened=0
for file in "$check"/backup/*.bundle.age; do
  [ -f "$file" ] || continue
  age -d -i "$KEY_FILE" -o "${file%.age}" "$file" || fail "не расшифровался $(basename "$file")"
  git clone --quiet --bare "${file%.age}" "${file%.bundle.age}.git" ||
    fail "из $(basename "${file%.age}") не клонируется стор"
  opened=$((opened + 1))
done
# The cabinet's dump is opened too, and its table of contents read: a dump that decrypts but does
# not restore is no backup. The plaintext stays in the temporary directory the trap removes.
dumped="дампа базы кабинета нет (кабинета на хосте нет)"
if [ -f "$check/backup/cabinet.pgdump.age" ]; then
  age -d -i "$KEY_FILE" -o "$check/cabinet.pgdump" "$check/backup/cabinet.pgdump.age" ||
    fail "не расшифровался дамп базы кабинета"
  if command -v pg_restore >/dev/null; then
    tables=$(pg_restore --list "$check/cabinet.pgdump" | grep -c 'TABLE DATA' || true)
    [ "$tables" -gt 0 ] || fail "в дампе базы кабинета нет данных таблиц"
    dumped="дамп базы кабинета читается, таблиц с данными: $tables"
  else
    dumped="дамп базы кабинета расшифрован; pg_restore на этой машине нет, содержимое не проверено"
  fi
fi
say "6/6 Бэкап прошёл; на этой машине расшифровано и склонировано сторов: $opened; $dumped"
