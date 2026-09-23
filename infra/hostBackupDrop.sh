#!/usr/bin/env bash
# A drop box on the store's host for another product's backups: one system user whose only key is
# locked into rrsync with write-only rights, one directory, and a daily clean-up of old copies.
#
# The product encrypts its backups before sending them (its owner holds the only key to open them),
# so the host keeps opaque files it can neither read nor has to. A key stolen from the product's
# server can add a copy and nothing else: it cannot read, overwrite or delete what is there, and it
# gets no shell. Old copies are removed by root, because the sender cannot.
#
# Runs on the OWNER'S machine; everything on the host happens in one ssh session. Idempotent: a
# second run with the same arguments changes nothing.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEFAULT_KEEP_DAYS=30
readonly DROP_ROOT=/srv/backups
readonly RRSYNC=/usr/bin/rrsync

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
product=""
key=""
keepDays="$DEFAULT_KEEP_DAYS"

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<TEXT
Завести на хосте стора приёмник копий соседнего продукта.

  ./infra/hostBackupDrop.sh --product <имя> --key "ssh-ed25519 <base64> [комментарий]"
                            [--keep-days 30] [--alias vibememory]

Учётка <имя>-backup без sudo и вне групп VibeMemory, каталог $DROP_ROOT/<имя> (700),
ключ заперт в rrsync только на запись, копии старше --keep-days дней удаляет root.
TEXT
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --product) product="${2:-}"; shift 2 ;;
    --key) key="${2:-}"; shift 2 ;;
    --keep-days) keepDays="${2:-}"; shift 2 ;;
    --alias) sshAlias="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) fail "неизвестный аргумент $1" ;;
  esac
done

case "$product" in ""|-*|*[!a-z0-9-]*) usage; fail "--product: строчные латинские буквы, цифры и дефис" ;; esac
case "$keepDays" in ""|*[!0-9]*) fail "--keep-days — целое число дней" ;; esac
[ "$keepDays" -gt 0 ] || fail "--keep-days — больше нуля"
# One line, an ed25519 key and an optional comment: the line becomes part of authorized_keys, where
# anything more would be an option.
case "$key" in
  *$'\n'*|*$'\r'*) fail "--key — одна строка" ;;
  "ssh-ed25519 "*) ;;
  *) fail "--key — открытый ключ ssh-ed25519" ;;
esac
keyBlob=$(printf '%s' "$key" | cut -d ' ' -f 2)
case "$keyBlob" in ""|*[!A-Za-z0-9+/=]*) fail "--key: второе поле — base64 ключа" ;; esac

say "Приёмник копий $product на $sshAlias: $DROP_ROOT/$product, хранение $keepDays дней"
# ssh joins its arguments into one command line: quoted, the key's spaces survive the trip.
ssh -o BatchMode=yes "$sshAlias" \
  "bash -s -- $(printf '%q ' "$product" "$key" "$keepDays" "$DROP_ROOT" "$RRSYNC")" <<'REMOTE'
set -euo pipefail
product="$1"; key="$2"; keepDays="$3"; dropRoot="$4"; rrsync="$5"
account="$product-backup"
drop="$dropRoot/$product"
home="/home/$account"

[ -x "$rrsync" ] || { echo "Ошибка: нет $rrsync — пакет rsync" >&2; exit 1; }

# The account: a shell, because a forced command runs through the user's shell and rrsync does
# not start under nologin; no password, no sudo, no group of VibeMemory.
if ! id "$account" >/dev/null 2>&1; then
  sudo useradd --system --create-home --home-dir "$home" --shell /bin/sh --user-group "$account"
fi
sudo usermod -p '*' "$account"
for group in vibememory vmaccess sudo; do
  if id -nG "$account" | tr ' ' '\n' | grep -qx "$group"; then
    echo "Ошибка: $account состоит в группе $group" >&2; exit 1
  fi
done
echo "1/4 Учётка $account: оболочка $(getent passwd "$account" | cut -d: -f7), без пароля, группы: $(id -nG "$account")"

sudo install -d -o root -g root -m 755 "$dropRoot"
sudo install -d -o "$account" -g "$account" -m 700 "$drop"
echo "2/4 Каталог $drop: $(sudo stat -c '%A %U:%G' "$drop")"

# Exactly one line: the product's key, locked into rrsync. -wo — write only, so a stolen key does
# not read old copies; -no-del and -no-overwrite — it neither deletes nor rewrites them.
line="command=\"$rrsync -wo -no-del -no-overwrite $drop\",restrict $key"
sudo install -d -o "$account" -g "$account" -m 700 "$home/.ssh"
current=$(sudo cat "$home/.ssh/authorized_keys" 2>/dev/null || true)
if [ "$current" != "$line" ]; then
  printf '%s\n' "$line" | sudo tee "$home/.ssh/authorized_keys" >/dev/null
fi
sudo chown "$account:$account" "$home/.ssh/authorized_keys"
sudo chmod 600 "$home/.ssh/authorized_keys"
echo "3/4 Ключ: $(sudo cat "$home/.ssh/authorized_keys" | wc -l | tr -d " ") строка, rrsync -wo -no-del -no-overwrite"

# Old copies go at night, by root: the sender's key cannot delete anything.
cron="/etc/cron.d/$product-backup-retention"
cronText="# Written by infra/hostBackupDrop.sh: copies of $product older than $keepDays days.
23 5 * * * root find $drop -type f -mtime +$keepDays -delete"
if [ "$(sudo cat "$cron" 2>/dev/null || true)" != "$cronText" ]; then
  printf '%s\n' "$cronText" | sudo tee "$cron" >/dev/null
  sudo chmod 644 "$cron"
fi
echo "4/4 Чистка: $cron, каждый день в 05:23, старше $keepDays дней"
REMOTE
