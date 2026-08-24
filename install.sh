#!/bin/sh
# cue installer.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/nkootstra/cue/main/install.sh | sh
#
# Environment:
#   CUE_INSTALL_DIR  target directory (default: ~/.local/bin)
#
# Flags:
#   --version vX.Y.Z  install a specific release (default: latest)
#   --dry-run         print what would be done without installing

set -eu

REPO="nkootstra/cue"
INSTALL_DIR="${CUE_INSTALL_DIR:-$HOME/.local/bin}"
VERSION=""
DRY_RUN=0

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            VERSION="${2#v}"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=1
            shift
            ;;
        *)
            echo "unknown option: $1" >&2
            exit 2
            ;;
    esac
done

# Map uname output to a release target triple.
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux) os_part="unknown-linux-gnu" ;;
    *)
        echo "unsupported operating system: $os" >&2
        exit 1
        ;;
esac
case "$arch" in
    arm64 | aarch64) arch_part="aarch64" ;;
    x86_64) arch_part="x86_64" ;;
    *)
        echo "unsupported architecture: $arch" >&2
        exit 1
        ;;
esac
TARGET="${arch_part}-${os_part}"

if [ -n "$VERSION" ]; then
    BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION}"
else
    BASE_URL="https://github.com/${REPO}/releases/latest/download"
fi
ASSET="cue-${TARGET}.tar.gz"

echo "cue installer"
echo "  platform:  ${TARGET}"
echo "  release:   ${VERSION:-latest}"
echo "  asset:     ${ASSET}"
echo "  install to: ${INSTALL_DIR}"

if [ "$DRY_RUN" = "1" ]; then
    echo "dry run: would download ${BASE_URL}/${ASSET}"
    exit 0
fi

command -v curl > /dev/null 2>&1 || {
    echo "curl is required" >&2
    exit 1
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "downloading..."
curl -fsSL -o "$tmp/$ASSET" "${BASE_URL}/${ASSET}"
curl -fsSL -o "$tmp/${ASSET}.sha256" "${BASE_URL}/${ASSET}.sha256"

echo "verifying checksum..."
if command -v shasum > /dev/null 2>&1; then
    echo "$(cat "$tmp/${ASSET}.sha256")" | shasum -a 256 -c - > /dev/null
else
    echo "$(cat "$tmp/${ASSET}.sha256")" | sha256sum -c - > /dev/null
fi

tar xzf "$tmp/$ASSET" -C "$tmp"

mkdir -p "$INSTALL_DIR"
mv "$tmp/cue-${TARGET}/cue" "$INSTALL_DIR/cue"
chmod +x "$INSTALL_DIR/cue"

echo
echo "installed $(INSTALL_DIR="$INSTALL_DIR" "$INSTALL_DIR/cue" --version) to $INSTALL_DIR/cue"
echo
echo "Next steps:"
echo "  1. Ensure $INSTALL_DIR is on your PATH"
echo "  2. Install FFmpeg if missing (brew install ffmpeg / apt install ffmpeg)"
echo "  3. Run: cue doctor"
echo "  4. Optionally: cue doctor --fix   (provisions the Python worker)"
