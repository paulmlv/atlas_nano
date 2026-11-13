#!/bin/sh
# Install atlas-nano from its GitHub Releases.
#
#   curl -fsSL https://github.com/paulmlv/atlas_nano/releases/latest/download/install.sh | sh
#
# ATLAS_NANO_VERSION picks a release, such as v0.1.0; the default is the latest.
# ATLAS_NANO_INSTALL_DIR picks the folder; the default is ~/.local/bin.
#
# Everything runs from main, called on the last line, so a download cut off midway runs nothing.
set -eu

repo="paulmlv/atlas_nano"

fail() {
    echo "atlas-nano install: $*" >&2
    exit 1
}

# The release target for this machine. Linux binaries are static, so any distribution works.
detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    # A shell under Rosetta reports x86_64 on Apple silicon, where the arm64 binary runs.
    if [ "$os" = Darwin ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
        arch=arm64
    fi
    case "$os/$arch" in
        Darwin/arm64) echo "aarch64-apple-darwin" ;;
        Linux/x86_64) echo "x86_64-unknown-linux-musl" ;;
        Linux/aarch64 | Linux/arm64) echo "aarch64-unknown-linux-musl" ;;
        *) fail "no prebuilt binary for $os $arch; build from source with cargo instead" ;;
    esac
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1"
    else
        fail "need sha256sum or shasum to check the download"
    fi
}

main() {
    version="${ATLAS_NANO_VERSION:-latest}"
    dir="${ATLAS_NANO_INSTALL_DIR:-$HOME/.local/bin}"
    target="$(detect_target)"

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
    curl -fsSL "$base/$archive.sha256" -o "$tmp/$archive.sha256" ||
        fail "could not download the checksum"

    expected="$(cut -d ' ' -f 1 "$tmp/$archive.sha256")"
    sum="$(sha256 "$tmp/$archive")"
    actual="${sum%% *}"
    case "$expected" in
        *[!0-9a-f]* | "") fail "the checksum file is not a SHA-256" ;;
    esac
    [ "${#expected}" -eq 64 ] || fail "the checksum file is not a SHA-256"
    [ "$expected" = "$actual" ] || fail "checksum mismatch for $archive"

    tar -xzf "$tmp/$archive" -C "$tmp"
    mkdir -p "$dir"
    install -m 755 "$tmp/atlas-nano" "$dir/atlas-nano"
    echo "Installed atlas-nano to $dir/atlas-nano"

    case ":$PATH:" in
        *":$dir:"*) ;;
        *) echo "Add $dir to your PATH to run atlas-nano from anywhere." ;;
    esac
}

main "$@"
