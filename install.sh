#!/bin/sh
# Install atlas-nano from its GitHub Releases.
#
#   curl -fsSL https://raw.githubusercontent.com/paulmlv/atlas_nano/main/install.sh | sh
#
# ATLAS_NANO_VERSION picks a release, such as v0.1.0; the default is the latest.
# ATLAS_NANO_INSTALL_DIR picks the folder; the default is ~/.local/bin.
set -eu

repo="paulmlv/atlas_nano"
version="${ATLAS_NANO_VERSION:-latest}"
dir="${ATLAS_NANO_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
    echo "atlas-nano install: $*" >&2
    exit 1
}

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) target="aarch64-apple-darwin" ;;
    Linux/x86_64) target="x86_64-unknown-linux-gnu" ;;
    Linux/aarch64 | Linux/arm64) target="aarch64-unknown-linux-gnu" ;;
    *) fail "no prebuilt binary for $(uname -s) $(uname -m); build from source with cargo instead" ;;
esac

if [ "$version" = latest ]; then
    base="https://github.com/$repo/releases/latest/download"
else
    base="https://github.com/$repo/releases/download/$version"
fi
archive="atlas-nano-$target.tar.gz"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $archive ($version)"
curl -fsSL "$base/$archive" -o "$tmp/$archive" || fail "could not download $base/$archive"
curl -fsSL "$base/$archive.sha256" -o "$tmp/$archive.sha256" || fail "could not download the checksum"

expected="$(cut -d ' ' -f 1 "$tmp/$archive.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$archive" | cut -d ' ' -f 1)"
else
    actual="$(shasum -a 256 "$tmp/$archive" | cut -d ' ' -f 1)"
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $archive"

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/atlas-nano" "$dir/atlas-nano"
echo "Installed atlas-nano to $dir/atlas-nano"

case ":$PATH:" in
    *":$dir:"*) ;;
    *) echo "Add $dir to your PATH to run atlas-nano from anywhere." ;;
esac
