#!/bin/sh
# Installs VibeMemory on macOS or Linux in one line:
#
#   curl -fsSL https://app.vibememory.ru/install.sh | sh
#
# Takes the latest release of this system from the host, checks its SHA-256 against the sum the host
# publishes beside it, unpacks it into a temporary directory and runs `vibememory install` from there:
# both programs go to ~/.vibememory/bin, which is put on PATH for new terminals. The temporary
# directory goes away; nothing is left anywhere else. A sum that does not match installs nothing.
#
# POSIX sh, curl and tar: nothing else is needed. VIBEMEMORY_DL names another release directory,
# for a test against a local copy.
set -eu

base="${VIBEMEMORY_DL:-https://app.vibememory.ru/dl/}"

fail() {
  printf 'VibeMemory: %s\n' "$*" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is needed"
command -v tar >/dev/null 2>&1 || fail "tar is needed"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
  *) fail "no release for $(uname -s) $(uname -m) yet" ;;
esac

version=$(curl -fsSL "${base}latest") || fail "the latest release could not be read from ${base}"
case "$version" in
  "" | *[!0-9A-Za-z.-]*) fail "the host answered no version" ;;
esac
archive="vibememory-$version-$target.tar.gz"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
cd "$work"

printf 'VibeMemory %s for %s\n' "$version" "$target"
curl -fsSLO "${base}$version/$archive" -O "${base}$version/$archive.sha256" ||
  fail "the archive could not be downloaded"

expected=$(cut -d' ' -f1 "$archive.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$archive" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$archive" | cut -d' ' -f1)
fi
[ -n "$expected" ] && [ "$expected" = "$actual" ] ||
  fail "the SHA-256 of $archive does not match the published one: nothing is installed"

mkdir unpacked
tar xzf "$archive" -C unpacked
./unpacked/vibememory install
