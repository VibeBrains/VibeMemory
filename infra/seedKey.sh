#!/usr/bin/env bash
# Seeds an SSH key for the VibeMemory host. Runs on the OWNER'S machine, never on the server.
#
# The account password is typed by the human into ssh's own prompt: this script never takes it
# from an argument, an environment variable or a file, and never stores it. The key passphrase
# (optional) is asked by ssh-keygen and is likewise not stored here.
#
# Idempotent: an existing key is reused, ssh-copy-id skips a key the server already has, and the
# host alias is added only once.
set -euo pipefail

readonly DEFAULT_USER=vm
readonly DEFAULT_HOST=vibememory.ru
readonly DEFAULT_PORT=22
readonly DEFAULT_KEY="$HOME/.ssh/id_ed25519_vibememory"
readonly SSH_ALIAS=vibememory

sshUser="${VIBEMEMORY_SSH_USER:-$DEFAULT_USER}"
sshHost="${VIBEMEMORY_SSH_HOST:-$DEFAULT_HOST}"
sshPort="${VIBEMEMORY_SSH_PORT:-$DEFAULT_PORT}"
keyPath="${VIBEMEMORY_SSH_KEY:-$DEFAULT_KEY}"

say() { printf '%s\n' "$*"; }
fail() {
  printf 'Ошибка: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<TEXT
Засеять SSH-ключ на хост VibeMemory. Запускается НА ДОМАШНЕЙ МАШИНЕ.

  ./infra/seedKey.sh [--user vm] [--host vibememory.ru] [--port 22] [--key ~/.ssh/id_ed25519_vibememory]

Что делает:
  1. создаёт ключ ed25519, если его ещё нет (парольную фразу спросит ssh-keygen);
  2. копирует публичную часть через ssh-copy-id — пароль учётки вводите вы, скрипт его не видит;
  3. добавляет алиас «${SSH_ALIAS}» в ~/.ssh/config;
  4. проверяет вход по ключу без пароля.

Переменные окружения: VIBEMEMORY_SSH_USER, VIBEMEMORY_SSH_HOST, VIBEMEMORY_SSH_PORT,
VIBEMEMORY_SSH_KEY.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --user) sshUser="${2:-}"; shift 2 ;;
    --host) sshHost="${2:-}"; shift 2 ;;
    --port) sshPort="${2:-}"; shift 2 ;;
    --key)  keyPath="${2:-}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) fail "Неизвестный аргумент: $1 (--help покажет список)" ;;
  esac
done

[ -n "$sshUser" ] || fail "Пустое имя пользователя"
[ -n "$sshHost" ] || fail "Пустой адрес сервера"
[ -n "$sshPort" ] || fail "Пустой порт"
[ -n "$keyPath" ] || fail "Пустой путь к ключу"

for tool in ssh ssh-keygen ssh-copy-id; do
  command -v "$tool" >/dev/null 2>&1 ||
    fail "Не найдена команда «$tool». На macOS: brew install ssh-copy-id"
done

say "Цель: ${sshUser}@${sshHost}:${sshPort}"
say "Ключ: ${keyPath}"
say ""

# 1. Key pair.
mkdir -p "$(dirname "$keyPath")"
chmod 700 "$(dirname "$keyPath")"
if [ -f "$keyPath" ]; then
  say "1/4 Ключ уже есть — использую существующий, не перезаписываю."
else
  say "1/4 Создаю ключ. Придумайте парольную фразу (Enter — без неё)."
  say "    Скрипт её не видит и нигде не сохраняет."
  ssh-keygen -t ed25519 -a 100 -C "vibememory@$(hostname -s)" -f "$keyPath"
fi
[ -f "${keyPath}.pub" ] || fail "Нет публичной части ${keyPath}.pub — удалите ${keyPath} и повторите"
say ""

# 2. Copy the public half. ssh-copy-id prompts for the password itself and skips a key the
# server already has, so re-running is harmless.
say "2/4 Копирую публичный ключ. Пароль учётки спросит ssh — вводите прямо в приглашении."
ssh-copy-id -i "${keyPath}.pub" -p "$sshPort" "${sshUser}@${sshHost}"
say ""

# 3. Host alias, so every later script writes just `ssh vibememory`.
config="${HOME}/.ssh/config"
touch "$config"
chmod 600 "$config"
if grep -qiE "^host[[:space:]]+${SSH_ALIAS}\b" "$config"; then
  say "3/4 Алиас «${SSH_ALIAS}» уже в ~/.ssh/config."
else
  say "3/4 Добавляю алиас «${SSH_ALIAS}» в ~/.ssh/config."
  # AddKeysToAgent/UseKeychain matter when the key has a passphrase: without them every later
  # non-interactive use (the tick, a script) fails with "Permission denied" right after the
  # server has already accepted the key — it is the signature that cannot be made, not the key
  # that is wrong. Measured 2026-09-07, and the message is misleading enough to be worth a line.
  cat >> "$config" <<EOF

Host ${SSH_ALIAS}
    HostName ${sshHost}
    User ${sshUser}
    Port ${sshPort}
    IdentityFile ${keyPath}
    IdentitiesOnly yes
    AddKeysToAgent yes
    UseKeychain yes
EOF
fi
say ""

# 4. Verify: key only, no password fallback, no interactive prompt.
say "4/4 Проверяю вход по ключу (пароль намеренно запрещён)…"
if ssh -i "$keyPath" -p "$sshPort" \
  -o IdentitiesOnly=yes -o BatchMode=yes -o PreferredAuthentications=publickey \
  "${sshUser}@${sshHost}" 'echo ok' | grep -qx ok; then
  say "    Готово: вход по ключу работает."
else
  fail "Вход по ключу не удался. Проверьте, что на сервере разрешён publickey-вход, и повторите."
fi

say ""

# A passphrase-protected key must be in the agent, or every non-interactive use fails.
if ! ssh-keygen -y -P "" -f "$keyPath" >/dev/null 2>&1; then
  if ssh-add -l 2>/dev/null | grep -q "$(ssh-keygen -lf "${keyPath}.pub" | awk '{print $2}')"; then
    say "Ключ с парольной фразой и уже в ssh-agent — неинтерактивные запуски будут работать."
  else
    say "Ключ защищён парольной фразой, а в ssh-agent его нет."
    say "Добавьте его один раз (фразу спросит ssh-add, скрипт её не видит):"
    say ""
    say "    ssh-add --apple-use-keychain ${keyPath}"
    say ""
    say "Без этого тик и скрипты будут получать «Permission denied» сразу после того, как"
    say "сервер принял ключ: подписать вход без фразы нечем."
  fi
  say ""
fi

say "Дальше: ./infra/hostBootstrap.sh — заводит bare-репозиторий стора на сервере."
