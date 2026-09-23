#!/usr/bin/env bash
# Publishes a release on the host: what the build machines left in ~/releases/incoming/<version>/
# is checked and moved to /srv/vibememory/releases/<version>/, which Caddy serves as /dl/, and
# index.json — the cabinet's download page — is rewritten from what is published.
#
# Runs on the OWNER'S machine; the work is one ssh session. Refuses the whole version, touching
# nothing published, when a sum does not match or an archive holds anything but the two binaries:
# vibememory and vibememory-mcp, with .exe for a Windows target.
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
version=""

fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --alias) sshAlias="${2:-}"; shift 2 ;;
    -h | --help) printf 'Проверить и опубликовать релиз на хосте.\n\n  ./infra/hostRelease.sh <версия> [--alias vibememory]\n'; exit 0 ;;
    *) [ -z "$version" ] || fail "лишний аргумент $1"; version="$1"; shift ;;
  esac
done
case "$version" in ""|*[!0-9A-Za-z.-]*) fail "версия ${version:-не задана} — только цифры, буквы, точки и дефисы" ;; esac

ssh -o BatchMode=yes "$sshAlias" "bash -s -- $(printf '%q' "$version")" <<'REMOTE'
set -euo pipefail
version="$1"
incoming="$HOME/releases/incoming/$version"
releases=/srv/vibememory/releases
[ -d "$incoming" ] || { echo "Ошибка: нет $incoming" >&2; exit 1; }
cd "$incoming"
archives=$(ls -1 vibememory-"$version"-*.tar.gz 2>/dev/null || true)
[ -n "$archives" ] || { echo "Ошибка: в $incoming нет архивов версии $version" >&2; exit 1; }

# every archive: its sum, and exactly the two binaries
for archive in $archives; do
  [ -f "$archive.sha256" ] || { echo "Ошибка: нет суммы $archive.sha256" >&2; exit 1; }
  sha256sum --quiet -c "$archive.sha256" || { echo "Ошибка: сумма $archive не сходится" >&2; exit 1; }
  target="${archive#vibememory-$version-}"; target="${target%.tar.gz}"
  case "$target" in *windows*) suffix=.exe ;; *) suffix="" ;; esac
  expected=$(printf '%s\n' "vibememory$suffix" "vibememory-mcp$suffix" | sort)
  listed=$(tar tzf "$archive" | sed 's|^\./||' | grep -v '/$' | sort)
  [ "$listed" = "$expected" ] ||
    { echo "Ошибка: в $archive не ровно два бинаря: $(printf '%s ' $listed)" >&2; exit 1; }
done

sudo install -d -o root -g root -m 755 "$releases" "$releases/$version"
for archive in $archives; do
  sudo install -o root -g root -m 644 "$archive" "$archive.sha256" "$releases/$version/"
done

# the index from what is published, every version: the cabinet only reads it
index=$(mktemp)
python3 - "$releases" > "$index" <<'PY'
import json, os, re, subprocess, sys
root = sys.argv[1]
index = {}
for version in sorted(os.listdir(root)):
    directory = os.path.join(root, version)
    if not os.path.isdir(directory):
        continue
    for name in sorted(os.listdir(directory)):
        match = re.fullmatch(r"vibememory-" + re.escape(version) + r"-(.+)\.tar\.gz", name)
        if not match:
            continue
        with open(os.path.join(directory, name + ".sha256")) as sums:
            digest = sums.read().split()[0]
        files = subprocess.run(["tar", "tzf", os.path.join(directory, name)], capture_output=True, text=True, check=True).stdout.split()
        index.setdefault(version, {})[match.group(1)] = {"archive": name, "sha256": digest, "files": sorted(f.removeprefix("./") for f in files)}
print(json.dumps(index, indent=2))
PY
sudo install -o root -g root -m 644 "$index" "$releases/index.json.new"
sudo mv -f "$releases/index.json.new" "$releases/index.json"
rm -f "$index"
rm -rf "$incoming"
echo "Опубликован $version: $(echo $archives | wc -w) архив(а), index.json переписан"
REMOTE
