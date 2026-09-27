#!/usr/bin/env bash
# Builds this version's release archives on the owner's Mac and sends them to the host's incoming
# directory, where infra/hostRelease.sh checks and publishes them.
#
# Per target: vibememory-<version>-<target>.tar.gz with both binaries at its root — `install` places
# the MCP server only from beside the running engine, so one without the other is no install — and
# <archive>.sha256 in sha256sum's format, which `shasum -a 256 -c` on macOS reads too. macOS arm64
# and x86_64 are built here; Linux x86_64 in a native arm64 container (this Docker has no amd64
# emulation — knowledge/design/hostMemoryServer.md); Windows here too, with mingw-w64 (the GNU target
# needs neither Visual Studio nor the Windows SDK, and its .exe asks only for Windows' own libraries —
# knowledge/toolchain/windowsFromMac.md).
# A copy of everything stays in /Volumes/Storage/Caches/VibeMemory/releases/<version>/.
set -euo pipefail

readonly ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly CACHE=/Volumes/Storage/Caches/VibeMemory/releases
readonly DEFAULT_ALIAS=vibememory
readonly MAC_TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
readonly LINUX_TARGET=x86_64-unknown-linux-gnu
readonly WINDOWS_TARGET=x86_64-pc-windows-gnu
readonly WINDOWS_LINKER=x86_64-w64-mingw32-gcc
readonly RUST_IMAGE=rust:1.97-bookworm
readonly PACKAGES="-p vibememory-cli -p vibememory-mcp"
# A lost SYN is retried instead of failing the run: the path to the host drops a connection now and
# then, and every step here is safe to repeat.
readonly SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=15 -o ConnectionAttempts=4)

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
upload=1

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --alias) sshAlias="${2:-}"; shift 2 ;;
    --no-upload) upload=0; shift ;;
    -h | --help)
      printf 'Собрать архивы релиза под macOS, Linux и Windows и отправить их в incoming/ на хост.\n\n  ./infra/releaseBuild.sh [--alias vibememory] [--no-upload]\n'
      exit 0 ;;
    *) fail "неизвестный аргумент $1" ;;
  esac
done

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml")
[ -n "$version" ] || fail "версия не найдена в Cargo.toml"
[ -z "$(git -C "$ROOT" status --porcelain -- crates Cargo.toml Cargo.lock)" ] ||
  fail "в крейтах незакоммиченные правки — релиз собирается из коммита"
out="$CACHE/$version"
mkdir -p "$out"

# pack <target> <directory with the binaries> <exe suffix>
pack() {
  local target="$1" dir="$2" suffix="$3" staging archive
  staging=$(mktemp -d)
  for binary in vibememory vibememory-mcp; do
    install -m 755 "$dir/$binary$suffix" "$staging/$binary$suffix"
  done
  archive="vibememory-$version-$target.tar.gz"
  # no extended attributes and no owner names: the archive unpacks the same for anyone
  COPYFILE_DISABLE=1 tar --no-xattrs --uid 0 --gid 0 -czf "$out/$archive" -C "$staging" \
    "vibememory$suffix" "vibememory-mcp$suffix"
  rm -rf "$staging"
  (cd "$out" && shasum -a 256 "$archive" > "$archive.sha256")
  say "   $archive"
}

say "Релиз $version"
for target in $MAC_TARGETS; do
  # shellcheck disable=SC2086
  cargo build --quiet --release $PACKAGES --target "$target" --manifest-path "$ROOT/Cargo.toml"
  pack "$target" "$ROOT/target/$target/release" ""
done
docker run --rm --platform linux/arm64 -v "$ROOT":/src:ro -v "$ROOT/target/linux":/build \
  -e CARGO_TARGET_DIR=/build -e CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
  -w /src "$RUST_IMAGE" sh -c "apt-get update -qq && apt-get install -y -qq gcc-x86-64-linux-gnu libc6-dev-amd64-cross >/dev/null \
  && rustup target add $LINUX_TARGET >/dev/null 2>&1 && cargo build --quiet --release $PACKAGES --target $LINUX_TARGET"
pack "$LINUX_TARGET" "$ROOT/target/linux/$LINUX_TARGET/release" ""
command -v "$WINDOWS_LINKER" >/dev/null || fail "нет $WINDOWS_LINKER: brew install mingw-w64"
rustup target add "$WINDOWS_TARGET" >/dev/null 2>&1
# shellcheck disable=SC2086
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$WINDOWS_LINKER" \
  cargo build --quiet --release $PACKAGES --target "$WINDOWS_TARGET" --manifest-path "$ROOT/Cargo.toml"
pack "$WINDOWS_TARGET" "$ROOT/target/$WINDOWS_TARGET/release" ".exe"

if [ "$upload" = 1 ]; then
  ssh -n "${SSH_OPTIONS[@]}" "$sshAlias" "mkdir -p \$HOME/releases/incoming/$version"
  scp -q -o ConnectTimeout=15 -o ConnectionAttempts=4 "$out"/*.tar.gz "$out"/*.sha256 \
    "$ROOT/infra/install.sh" "$ROOT/infra/install.ps1" "$sshAlias:releases/incoming/$version/"
  say "Отправлено в ~/releases/incoming/$version/ — дальше ./infra/hostRelease.sh $version"
fi
