#!/usr/bin/env bash
# Prepares the store's host: service accounts and their directories, the owner's bare repository
# with the permissions the memory server needs, nightly repacking, sshd rules, and optionally a
# post-receive hook that mirrors every push to a private repository at any git host.
#
# Runs on the OWNER'S machine over the key seeded by seedKey.sh; each server step is one ssh
# session with a here-document, and infra/storeInit.sh and infra/storeRepack.sh are copied to the
# host first.
#
# Idempotent: accounts, directories and permissions are changed only where they differ, config
# values and files are rewritten only when their text differs, and sshd rules are applied under a
# rollback timer only when they change. Nothing is ever deleted.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_PATH=vibememory/store.git
readonly DEFAULT_BRANCH=main
readonly SCRIPTS_LOCAL="$(dirname "$0")"
readonly BIN_REMOTE=/srv/vibememory/bin
# The one source of repository settings, and the nightly repacking of the owner's store and of
# every team store: both are run on the host from here.
readonly HOST_SCRIPTS="storeInit.sh storeRepack.sh"
readonly STORE_INIT_REMOTE="$BIN_REMOTE/storeInit.sh"
readonly STORE_REPACK_REMOTE="$BIN_REMOTE/storeRepack.sh"

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
repoPath="${VIBEMEMORY_REPO_PATH:-$DEFAULT_PATH}"
branch="${VIBEMEMORY_BRANCH:-$DEFAULT_BRANCH}"
mirror="${VIBEMEMORY_MIRROR:-}"

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Подготовить хост стора: служебные учётки, bare-репозиторий владельца с правами для сервера памяти,
ночную упаковку и правила sshd.

  ./infra/hostBootstrap.sh [--alias vibememory] [--path vibememory/store.git]
                           [--branch main] [--mirror storeMirror:owner/repo.git]

  --mirror  необязателен: если задан, ставится хук post-receive, который после каждого push
            делает \`git push --mirror\` в этот репозиторий. Ключ и ssh-алиас на сервере заводит
            ./infra/mirrorSetup.sh — вызывать этот скрипт с --mirror руками обычно не нужно.

Правила sshd меняются под страховкой: если новое соединение по ключу не подтвердится за 180 секунд,
сервер сам вернёт прежние файлы.

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
for script in $HOST_SCRIPTS; do
  [ -f "$SCRIPTS_LOCAL/$script" ] || fail "Нет $SCRIPTS_LOCAL/$script"
done

ssh -o BatchMode=yes "$sshAlias" 'echo ok' >/dev/null 2>&1 ||
  fail "Сервер не пускает по ключу. Сначала ./infra/seedKey.sh"

say "Сервер: $sshAlias, репозиторий: ~/$repoPath, ветка: $branch"
[ -n "$mirror" ] && say "Зеркало: $mirror" || say "Зеркало: не настраивается (--mirror не задан)"
say ""

# 1. Accounts and directories. `bash -s` reads the script from stdin, so the quoting rules are the
# ordinary ones and nothing needs escaping twice.
ssh -o BatchMode=yes "$sshAlias" 'bash -s' <<'REMOTE'
set -euo pipefail
# git is not on a fresh Debian or Ubuntu image, and every later step needs it
if ! command -v git >/dev/null 2>&1; then
  sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq git >/dev/null
  echo "1/6 git установлен: $(git --version)"
fi

# Three accounts, each able to do exactly its job:
#   vm    — the owner: sudo, keys, the personal store;
#   vmgit — the memory server and the team repositories: no sudo, no password, no terminal;
#   vmcab — the cabinet: no sudo, no access to any repository.
# Group `vibememory` shares the repositories (vm, vmgit); group `vmaccess` shares the directory
# where the cabinet publishes rights and the host reports back (vmcab, vmgit).
for group in vibememory vmaccess; do
  getent group "$group" >/dev/null || { sudo groupadd --system "$group"; echo "1/6 Группа $group создана"; }
done
if ! id vmgit >/dev/null 2>&1; then
  sudo useradd --system --create-home --home-dir /home/vmgit --shell /bin/bash --user-group vmgit
  echo "1/6 Учётка vmgit создана"
fi
if ! id vmcab >/dev/null 2>&1; then
  sudo useradd --system --create-home --home-dir /home/vmcab --shell /usr/sbin/nologin --user-group vmcab
  echo "1/6 Учётка vmcab создана"
fi
# `*` and not a locked `!`: a locked account may be refused by sshd even with a valid key, and
# vmgit is reached only by key. `*` matches no password at all.
for user in vmgit vmcab; do
  [ "$(sudo getent shadow "$user" | cut -d: -f2)" = "*" ] || sudo usermod -p '*' "$user"
done
member() { id -nG "$1" | tr ' ' '\n' | grep -qx "$2"; }
member vm vibememory || sudo usermod -aG vibememory vm
member vmgit vibememory || sudo usermod -aG vibememory vmgit
member vmgit vmaccess || sudo usermod -aG vmaccess vmgit
member vmcab vmaccess || sudo usermod -aG vmaccess vmcab

# owner:group:mode for each directory; created when missing, corrected when different. Looked at
# with sudo: some of these live in homes the owner's account cannot enter (vmgit's is 700).
place() {
  local path="$1" owner="$2" group="$3" mode="$4"
  sudo test -d "$path" || sudo mkdir -p "$path"
  [ "$(sudo stat -c %U:%G "$path")" = "$owner:$group" ] || sudo chown "$owner:$group" "$path"
  [ "$(sudo stat -c %a "$path")" = "$mode" ] || sudo chmod "$mode" "$path"
}
place /srv/vibememory root root 755
place /srv/vibememory/bin root root 755
# setgid: every team repository inherits the group, whoever creates it.
place /srv/vibememory/teams vmgit vibememory 2770
# setgid for the group and the sticky bit on top: the cabinet and the memory server both write here,
# and neither may replace the other's file — the cabinet publishes rights, the host reports back.
place /srv/vibememory/access root vmaccess 3770
# The keys of vmgit are written by the application of the access snapshot alone, as vmgit.
place /home/vmgit/.ssh vmgit vmgit 700
echo "1/6 Учётки vmgit и vmcab, группы vibememory и vmaccess, каталоги /srv/vibememory на месте"
REMOTE

# 2. The shared scripts go to the host: the settings every store is initialised with, and the
# nightly repacking of the owner's store and of the team stores.
for script in $HOST_SCRIPTS; do
  ssh -o BatchMode=yes "$sshAlias" "tmp=\$(mktemp); cat > \"\$tmp\";
    if sudo cmp -s \"\$tmp\" $BIN_REMOTE/$script; then echo '2/6 $script на хосте совпадает';
    else sudo install -o root -g root -m 755 \"\$tmp\" $BIN_REMOTE/$script; echo '2/6 $script записан на хост'; fi;
    rm -f \"\$tmp\"" < "$SCRIPTS_LOCAL/$script"
done

# 3. The personal store: settings from storeInit.sh, then permissions for the memory server.
# ssh glues its arguments into one command line for the remote shell, so an empty argument
# disappears and every argument after it moves one place left. Each one is quoted for that shell.
ssh -o BatchMode=yes "$sshAlias" "bash -s -- $(printf '%q ' "$repoPath" "$branch" "$STORE_INIT_REMOTE" "$STORE_REPACK_REMOTE" "$mirror")" <<'REMOTE'
set -euo pipefail
repoPath="${1:?путь репозитория не передан}"
branch="${2:?ветка не передана}"
storeInit="${3:?путь storeInit.sh не передан}"
repack="${4:?путь storeRepack.sh не передан}"
mirror="${5:-}"
repo="$HOME/$repoPath"
parent="$(dirname "$repo")"

[ -d "$repo" ] && echo "3/6 Репозиторий уже есть: ~/$repoPath" || echo "3/6 Создаю bare-репозиторий ~/$repoPath"
mkdir -p "$parent"
# `--adopted`: the owner's own store gets the common settings only; team rules are not his.
"$storeInit" "$repo" --adopted --branch "$branch"

# The store holds transcripts of every session: prompts, code, file contents. It is shared with
# the memory server's account through the group `vibememory` and with nobody else.
#
# The group may write exactly where the server writes a record — loose objects under `objects/`,
# the lock and rename that move `refs/heads/main` under `refs/` — and nowhere else. `config` and
# `hooks/` are executed by the owner's own git (the nightly repack, a push, the mirror hook), and
# the owner has sudo: group write on them would turn a hole in an internet-facing server into root
# on the host. The store directory itself stays 2750 for the same reason — write access to it lets
# a file be replaced by rename. It also means the server cannot delete refs (that locks
# `packed-refs` in the store directory); it only ever moves `main` forward.
#
# `core.sharedRepository` covers only files created from now on, so existing ones are brought over
# here. Only what differs is touched, so a second run leaves every mode and ctime as it was.
sudo find "$repo" ! -group vibememory -exec chgrp -h vibememory {} +
sudo find "$repo" -type d ! -perm -2050 -exec chmod g+rxs {} +
sudo find "$repo" -type f ! -perm -g+r -exec chmod g+r {} +
sudo find "$repo/objects" "$repo/refs" -type d ! -perm -g+w -exec chmod g+w {} +
sudo find "$repo" \( -path "$repo/objects" -o -path "$repo/refs" \) -prune -o -perm -g+w -exec chmod g-w {} +
# The way down to the store: the group may pass through the owner's home and the store's parent,
# not list them.
for dir in "$HOME" "$parent"; do
  [ "$(stat -c %G "$dir")" = vibememory ] || sudo chgrp vibememory "$dir"
  [ "$(stat -c %a "$dir")" = 710 ] || sudo chmod 710 "$dir"
done
# Hand-typed commands in the manuals use `git -C` under vmgit; git refuses that for a repository
# owned by another account unless the path is declared safe. Code never needs this: it uses
# `--git-dir`, which is not checked.
git config --system --get-all safe.directory 2>/dev/null | grep -Fxq "$repo" ||
  sudo git config --system --add safe.directory "$repo"
echo "4/6 Права стора: группа vibememory, запись группе только в objects/ и refs/; путь к нему — 710; safe.directory для $repo"

# Nightly repacking, incremental: storeRepack.sh, installed in step 2, says why it is not `git gc`
# and refuses to start without room
# A systemd timer, as for the team stores: cloud images of Debian 13 come without cron
# Earlier bootstraps put it into the owner's crontab; those lines go once the timer is in place,
# Or the store would be repacked twice a night
unitDir=/etc/systemd/system
service="[Unit]
Description=VibeMemory: nightly repacking of the owner's store

[Service]
Type=oneshot
User=$(id -un)
ExecStart=$repack $repo"
timer="[Unit]
Description=VibeMemory: repack the owner's store every night

[Timer]
OnCalendar=*-*-* 04:17:00
Persistent=true

[Install]
WantedBy=timers.target"
changed=0
for unit in vibememory-repack.service vibememory-repack.timer; do
  case "$unit" in *.service) text="$service" ;; *) text="$timer" ;; esac
  if [ "$(sudo cat "$unitDir/$unit" 2>/dev/null || true)" != "$text" ]; then
    printf '%s\n' "$text" | sudo tee "$unitDir/$unit" >/dev/null
    changed=1
  fi
done
[ "$changed" = 0 ] || sudo systemctl daemon-reload
sudo systemctl enable --now --quiet vibememory-repack.timer
# `set -e` kills the script on the first non-zero status, and both `crontab -l` (no crontab yet)
# And `grep -v` (nothing left) return one legitimately: hence `|| true`
if command -v crontab >/dev/null 2>&1; then
  current="$(crontab -l 2>/dev/null || true)"
  kept="$(printf '%s\n' "$current" | grep -Fv "$repoPath gc" | grep -Fv "storeRepack.sh" || true)"
  if [ "$kept" != "$current" ]; then
    if [ -n "$kept" ]; then printf '%s\n' "$kept" | crontab -; else crontab -r; fi
    echo "5/6 Прежняя строка упаковки убрана из crontab: её место занял таймер"
  fi
fi
echo "5/6 Ночная упаковка — таймер vibememory-repack.timer (04:17, частичная, журнал: journalctl -t vibememory-repack)"

hook="$repo/hooks/post-receive"
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
  if printf '%s' "$wanted" | cmp -s - "$hook"; then
    echo "5/6 Хук зеркала уже стоит и совпадает"
  else
    printf '%s' "$wanted" > "$hook"
    chmod +x "$hook"
    echo "5/6 Хук зеркала записан"
  fi
else
  echo "5/6 Хук зеркала не трогаю (--mirror не задан; его ставит ./infra/mirrorSetup.sh)"
fi
REMOTE

# 4. sshd rules. Two drop-ins: the host-wide hardening, and the rules of the memory server's
# account. Applied only when their text differs, and then under a rollback timer: the change stays
# only if a NEW key login is seen by sshd after the timer was armed (serverHardeningPrompt.md, step 2).
armed=$(ssh -o BatchMode=yes "$sshAlias" 'bash -s' <<'REMOTE'
set -euo pipefail
dir=/etc/ssh/sshd_config.d
bak=/run/vibememory-sshd.bak
hardening=10-vibememory-hardening.conf
service=20-vibememory-vmgit.conf
# The file name must sort before 50-cloud-init.conf: for sshd the first value seen wins, and cloud
# images ship that file with PasswordAuthentication yes.
# KexAlgorithms: the provider's image pins a list without post-quantum exchanges in sshd_config itself
# The drop-ins are included above that line, so this list wins
# No NIST curves in it: Apple's ssh offers ecdh-sha2-nistp256 first and would keep choosing it
# A client without post-quantum support falls back to curve25519, which every OpenSSH since 6.5 has
hardeningText='PermitRootLogin prohibit-password
PasswordAuthentication no
KbdInteractiveAuthentication no
MaxAuthTries 3
KexAlgorithms mlkem768x25519-sha256,sntrup761x25519-sha512,sntrup761x25519-sha512@openssh.com,curve25519-sha256,curve25519-sha256@libssh.org'
# Ends with `Match all`: drop-ins are included at the top of sshd_config, and without it every
# directive after the Include would apply to vmgit alone.
serviceText='# Written by infra/hostBootstrap.sh. The memory server account: keys only, from the one file the
# application of the access snapshot writes; no terminal, no forwarding of any kind.
Match User vmgit
    AuthorizedKeysFile .ssh/authorized_keys
    PasswordAuthentication no
    KbdInteractiveAuthentication no
    PermitTTY no
    AllowTcpForwarding no
    AllowStreamLocalForwarding no
    AllowAgentForwarding no
    X11Forwarding no
    PermitTunnel no
Match all'

if [ "$(sudo cat "$dir/$hardening" 2>/dev/null || true)" = "$hardeningText" ] &&
   [ "$(sudo cat "$dir/$service" 2>/dev/null || true)" = "$serviceText" ]; then
  echo UNCHANGED
  exit 0
fi
if sudo systemctl list-timers ssh-guard.timer --no-pager | grep -q ssh-guard; then
  echo "Ошибка: висит прошлая страховка ssh-guard — дождитесь её" >&2
  exit 1
fi
[ "$(sudo sshd -T | awk '$1=="loglevel"{print $2}')" != QUIET ] || {
  echo "Ошибка: LogLevel QUIET — строк Accepted не будет, откат сработал бы всегда" >&2
  exit 1
}

sudo rm -rf "$bak"; sudo mkdir -p "$bak"
for file in "$hardening" "$service"; do
  [ -f "$dir/$file" ] && sudo cp -p "$dir/$file" "$bak/$file"
done
sudo rm -f /run/ssh-ok
arm=$(date +%s)
# The guard restores what was there before (or removes what was not) unless /run/ssh-ok names a
# connection that sshd accepted after arming — something an old session cannot produce.
printf '%s\n' '#!/bin/sh' \
  "marker=\$(cat /run/ssh-ok 2>/dev/null)" \
  "if [ -n \"\$marker\" ] && journalctl -u ssh --since=@$arm -q | grep 'Accepted ' | grep -qF -- \"\$marker\"; then" \
  "  logger -t ssh-guard 'confirmed, kept'" \
  "else" \
  "  for f in $hardening $service; do" \
  "    if [ -f $bak/\$f ]; then cp -p $bak/\$f $dir/\$f; else rm -f $dir/\$f; fi" \
  "  done" \
  "  systemctl reload ssh; logger -t ssh-guard 'rolled back'" \
  "fi" \
  "rm -f /run/ssh-ok" | sudo tee "$bak/guard.sh" >/dev/null
sudo systemd-run --unit=ssh-guard --collect --on-active=180 --timer-property=AccuracySec=1s \
  sh "$bak/guard.sh" >/dev/null 2>&1

printf '%s\n' "$hardeningText" | sudo tee "$dir/$hardening" >/dev/null
printf '%s\n' "$serviceText" | sudo tee "$dir/$service" >/dev/null
if ! sudo sshd -t; then
  sudo systemctl stop ssh-guard.timer 2>/dev/null || true
  sudo sh "$bak/guard.sh"
  echo "Ошибка: sshd -t отверг правила — вернул прежние" >&2
  exit 1
fi
reloaded=$(date +%s)
sudo systemctl reload ssh
# `reload` returns once the signal is sent; the journal line may land a moment later.
sleep 1
sudo journalctl -u ssh --since=@"$reloaded" --no-pager | grep -q 'Received SIGHUP' ||
  { echo "Ошибка: sshd не подтвердил reload в журнале — страховка откатит через 180 с" >&2; exit 1; }
echo "ARMED $arm"
REMOTE
)

if [ "$armed" = UNCHANGED ]; then
  say "6/6 Правила sshd уже стоят и совпадают"
else
  arm="${armed#ARMED }"
  # A new connection, not a multiplexed one: ControlPath=none forces a fresh key login, and only
  # that login may confirm the change.
  ssh -o ControlPath=none -o BatchMode=yes "$sshAlias" \
    'set -- $SSH_CONNECTION; printf "from %s port %s " "$1" "$2" | sudo tee /run/ssh-ok >/dev/null' ||
    fail "Новое соединение не прошло — страховка вернёт прежние правила sshd через 180 с"
  say "6/6 Правила sshd применены; новое соединение подтверждено — жду окно страховки (до 3 мин)"
  ssh -o BatchMode=yes "$sshAlias" 'bash -s' -- "$arm" <<'REMOTE'
set -euo pipefail
arm="$1"
until ! sudo systemctl list-timers ssh-guard.timer --no-pager | grep -q ssh-guard; do sleep 5; done
if sudo journalctl -t ssh-guard --since=@"$arm" --no-pager -o cat | grep -q 'rolled back'; then
  echo "Ошибка: страховка откатила правила sshd — новое соединение не подтвердилось" >&2
  exit 1
fi
echo "6/6 Страховка сняла себя сама, правила остались:"
for user in vm vmgit; do
  printf '    %-6s %s\n' "$user" "$(sudo sshd -T -C "user=$user,host=x,addr=192.0.2.1" |
    awk '$1 ~ /^(passwordauthentication|permittty|allowtcpforwarding|permitrootlogin)$/ {printf "%s=%s ", $1, $2}')"
done
printf '    %-6s %s\n' kex "$(sudo sshd -T | awk '$1 == "kexalgorithms" {print $2}')"
REMOTE
fi

say ""
say "Готово. Дальше: ./infra/connectStore.sh — привязать локальный стор к серверу и запушить."
