#!/usr/bin/env bash
# Puts a directory of static pages on the host as the demo site: /srv/vibememory/demo/<build>, then the
# `current` link is switched in one rename, so a visitor never sees half a page. The last three builds
# stay for a rollback by hand. Caddy serves `current` for the demo domain — the site block is in
# infra/Caddyfile.tmpl and is put there by infra/caddyApply.sh.
#
# The pages are not product code and live outside this repository, so a build is named by its content:
# the same pages give the same build.
#
# Runs on the OWNER'S machine: ./infra/demoDeploy.sh <directory with index.html> [--alias vibememory]
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly DEMO=/srv/vibememory/demo
readonly KEEP=3
readonly SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=15 -o ConnectionAttempts=4)

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
source=""
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --alias) sshAlias="${2:-}"; shift 2 ;;
    -h | --help)
      printf 'Выложить каталог статических страниц на демо-сайт.\n\n  ./infra/demoDeploy.sh <каталог с index.html> [--alias %s]\n' "$DEFAULT_ALIAS"
      exit 0 ;;
    -*) fail "неизвестный аргумент $1" ;;
    *) [ -z "$source" ] || fail "каталог указан дважды"; source="$1"; shift ;;
  esac
done

[ -n "$source" ] || fail "не указан каталог со страницами"
[ -f "$source/index.html" ] || fail "в $source нет index.html"

# The build is named by the names and contents of the pages, not by the archive: an archive also carries
# file times, and touching a page would make a new build of the same pages.
files=$(cd "$source" && find . -type f ! -name '.DS_Store' | LC_ALL=C sort)
build=$(cd "$source" && printf '%s\n' "$files" | while read -r file; do shasum -a 256 "$file"; done |
  shasum -a 256 | cut -c1-12)
# macOS extended attributes stay home: GNU tar on the host would warn on every file
archive=$(mktemp)
trap 'rm -f "$archive"' EXIT
(cd "$source" && printf '%s\n' "$files" | COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -czf - -T - > "$archive")

printf 'Выкладываю демо (%s)\n' "$build"
# the script travels as an argument, the pages as stdin
readonly REMOTE='set -euo pipefail
build=$1 demo=$2 keep=$3
staging=$(mktemp -d)
trap '\''rm -rf "$staging"'\'' EXIT
tar -xzf - -C "$staging"
sudo install -d -o root -g root -m 755 "$demo"
sudo rm -rf "$demo/$build"
sudo cp -r "$staging" "$demo/$build"
sudo chown -R root:root "$demo/$build"
sudo find "$demo/$build" -type d -exec chmod 755 {} + -o -type f -exec chmod 644 {} +
sudo ln -sfn "$build" "$demo/current.new"
sudo mv -T "$demo/current.new" "$demo/current"
# the newest builds stay, the rest go; `current` is never among the removed
ls -1t "$demo" | grep -vx -e current -e "$build" | tail -n +"$keep" | while read -r old; do sudo rm -rf "${demo:?}/$old"; done
echo "Выложена сборка $build"
'
ssh "${SSH_OPTIONS[@]}" "$sshAlias" "bash -c $(printf '%q' "$REMOTE") deploy $(printf '%q' "$build") $DEMO $KEEP" < "$archive"
