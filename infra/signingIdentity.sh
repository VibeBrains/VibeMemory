#!/usr/bin/env bash
# Helps the owner create a stable code signing identity, so macOS stops asking for the same
# permission after every rebuild of the engine.
#
# The certificate is created by the owner, in their own keychain, with their own password: this
# script does not — and must not — handle either. It checks what exists, says exactly what to do,
# and writes the identity into config.json once it is there.
set -euo pipefail

readonly DEFAULT_NAME='VibeMemory Local'
name="${1:-$DEFAULT_NAME}"
config="${VIBEMEMORY_DIR:-$HOME/.vibememory}/config.json"

say() { printf '%s\n' "$*"; }

[ "$(uname -s)" = Darwin ] || {
  say "Это только для macOS: на других системах разрешения не привязаны к подписи."
  exit 0
}

# Not `security find-identity -v`: it lists only identities the system *trusts*, and a
# self-signed root is not trusted — while `codesign` signs with it perfectly well. Checked live
# 2026-09-08: find-identity said "0 valid identities", codesign produced Authority=VibeMemory
# Local. So the question is whether the certificate exists at all, by its label.
if security find-certificate -a 2>/dev/null | grep -Fq "\"labl\"<blob>=\"$name\""; then
  say "Сертификат «$name» уже есть."
else
  cat <<TEXT
Сертификата «$name» нет. Создайте его — это делается один раз и требует вашего пароля,
поэтому шаг за вами:

  1. Откройте «Связка ключей» (Keychain Access).
  2. Меню «Связка ключей» → «Ассистент сертификации» → «Создать сертификат…».
  3. Имя: $name
     Тип идентификации: Самоподписанный корневой (Self Signed Root)
     Тип сертификата: Подпись кода (Code Signing)
  4. Создать → Готово.

Затем запустите этот скрипт снова.
TEXT
  exit 1
fi

if [ ! -f "$config" ]; then
  say "Конфига нет: $config — сначала vibememory install."
  exit 1
fi

if grep -q '"signingIdentity"' "$config"; then
  say "В конфиге уже задан signingIdentity — правьте вручную, если нужно другое имя."
else
  # A minimal, dependency-free edit: insert the key after the opening brace. The engine reads
  # JSON, so the file must stay valid — checked immediately below.
  tmp="$config.new"
  awk -v value="$name" 'NR==1 && /^\{/ { print "{"; printf "  \"signingIdentity\": \"%s\",\n", value; next } { print }' "$config" > "$tmp"
  python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$tmp" 2>/dev/null || {
    rm -f "$tmp"
    say "Не удалось безопасно вписать значение — добавьте вручную:"
    say "  \"signingIdentity\": \"$name\""
    exit 1
  }
  mv "$tmp" "$config"
  say "В конфиг вписано: signingIdentity = $name"
fi

say ""
say "Дальше: vibememory install — он подпишет установленный бинарь и проверит, что тот запускается."
say "Затем один раз разрешите доступ в диалоге macOS; больше он не появится."
