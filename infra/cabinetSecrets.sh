#!/usr/bin/env bash
# Puts the owner's own values into the cabinet's .env on the host: the Resend key the cabinet sends
# mail with, the Telegram bot and chat urgent host alerts go to, and the support page's link.
#
# Runs on the OWNER'S machine, in a terminal. Secrets are read without echo and travel to the host
# through ssh's stdin — never as an argument, never into a file here, never printed; the host merges
# them into its .env (0600, vmcab) and restarts the cabinet. An empty answer keeps what is there.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
# A lost SYN is retried instead of failing the run: the path to the host drops a connection now and
# then, and every step here is safe to repeat.
readonly SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=15 -o ConnectionAttempts=4)
sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
case "${1:-}" in
  --alias) sshAlias="${2:-}" ;;
  -h | --help)
    printf 'Внести ключ Resend, бота Telegram и ссылку «поддержать» в .env кабинета на хосте.\n\n  ./infra/cabinetSecrets.sh [--alias vibememory]\n\nПустой ответ оставляет прежнее значение.\n'
    exit 0 ;;
  "") ;;
  *) printf 'Ошибка: неизвестный аргумент %s\n' "$1" >&2; exit 1 ;;
esac
[ -t 0 ] || { printf 'Ошибка: нужен терминал — значения вводятся руками\n' >&2; exit 1; }

ask() { # ask <variable> <prompt> <hidden>
  local value
  if [ "$3" = hidden ]; then
    read -rsp "$2: " value; printf '\n' >&2
  else
    read -rp "$2: " value
  fi
  case "$value" in *[[:space:]]*) printf 'Ошибка: в значении %s есть пробел или перевод строки\n' "$1" >&2; exit 1 ;; esac
  [ -z "$value" ] || lines+=("$1=$value")
}

lines=()
ask RESEND_API_KEY "Ключ Resend (re_…)" hidden
ask FROM_EMAIL_ADDRESS "Адрес отправителя писем (домен подтверждён в Resend)" visible
ask TELEGRAM_BOT_TOKEN "Токен бота Telegram" hidden
ask TELEGRAM_CHAT_ID "Чат Telegram для тревог (число)" visible
[ "${#lines[@]}" -gt 0 ] || { printf 'Ничего не введено — .env не тронут\n'; exit 0; }

printf '%s\n' "${lines[@]}" | ssh "${SSH_OPTIONS[@]}" "$sshAlias" 'bash -c '"'"'
set -euo pipefail
env=/home/vmcab/cabinet/.env
sudo test -f "$env" || { echo "Ошибка: нет $env — сначала hostCabinet.sh" >&2; exit 1; }
merged=$(mktemp)
trap "rm -f \"$merged\"" EXIT
chmod 600 "$merged"
declare -A given=()
while IFS= read -r line; do given["${line%%=*}"]="${line#*=}"; done
while IFS= read -r line; do
  key="${line%%=*}"
  if [ -n "${given[$key]+set}" ]; then printf "%s=%s\n" "$key" "${given[$key]}"; unset "given[$key]"; else printf "%s\n" "$line"; fi
done < <(sudo cat "$env") > "$merged"
for key in "${!given[@]}"; do printf "%s=%s\n" "$key" "${given[$key]}" >> "$merged"; done
sudo install -o vmcab -g vmcab -m 600 "$merged" "$env"
sudo systemctl restart vibememory-cabinet
echo "Записано, кабинет перезапущен ($(systemctl is-active vibememory-cabinet))"
'"'"''
printf 'Внесены: %s\n' "$(printf '%s\n' "${lines[@]}" | cut -d= -f1 | paste -sd ' ' -)"
