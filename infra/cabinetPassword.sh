#!/usr/bin/env bash
# Sets the cabinet owner's password — the one account the snapshot import makes without one.
#
# Runs on the OWNER'S machine, in a terminal: the password is typed on the host's terminal through
# `ssh -t`, twice, without echo. It never passes through an argument, a file or this script, and a
# session whose transcript is synced never sees it — which is why nobody but the owner runs this.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
case "${1:-}" in
  --alias) sshAlias="${2:-}" ;;
  -h | --help) printf 'Задать пароль владельца кабинета (вводится на терминале хоста, без эха).\n\n  ./infra/cabinetPassword.sh [--alias vibememory]\n'; exit 0 ;;
  "") ;;
  *) printf 'Ошибка: неизвестный аргумент %s\n' "$1" >&2; exit 1 ;;
esac
[ -t 0 ] || { printf 'Ошибка: нужен терминал — пароль вводится руками\n' >&2; exit 1; }

ssh -t "$sshAlias" "sudo -u vmcab -H bash -c 'cd /home/vmcab/cabinet && NODE_ENV=production /home/vmcab/.bun/bin/bun run owner:password'"
