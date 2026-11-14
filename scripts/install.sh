#!/bin/sh
set -eu

REPO="subsumptive/blocks"
VERSION="${SUB_VERSION:-latest}"
INSTALL_DIR="${SUB_INSTALL_DIR:-$HOME/.local/bin}"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      VERSION="$2"
      shift 2
      ;;
    --install-dir)
      INSTALL_DIR="$2"
      shift 2
      ;;
    -h|--help)
      echo "Usage: install.sh [--version <tag>] [--install-dir <dir>]"
      exit 0
      ;;
    *)
      echo "error: unknown option '$1'" >&2
      exit 1
      ;;
  esac
done

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64|amd64) TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
      *) echo "error: unsupported Linux architecture '$ARCH'" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64) TARGET="x86_64-apple-darwin" ;;
      arm64) TARGET="aarch64-apple-darwin" ;;
      *) echo "error: unsupported macOS architecture '$ARCH'" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "error: unsupported operating system '$OS'" >&2
    exit 1
    ;;
esac

if [ "$VERSION" = "latest" ]; then
  BASE_URL="https://github.com/$REPO/releases/latest/download"
  ARCHIVE="sub-latest-${TARGET}.tar.gz"
  # Resolve actual latest tag via GitHub redirect if needed
  TAG_URL="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")"
  VERSION="${TAG_URL##*/}"
fi

ARCHIVE="sub-${VERSION}-${TARGET}.tar.gz"
BASE_URL="https://github.com/$REPO/releases/download/${VERSION}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Downloading Subsumptive Blocks ${VERSION} (${TARGET})..."
curl -fsSL "${BASE_URL}/${ARCHIVE}" -o "${TMP_DIR}/${ARCHIVE}"
curl -fsSL "${BASE_URL}/SHA256SUMS.txt" -o "${TMP_DIR}/SHA256SUMS.txt"

EXPECTED_SHA="$(grep " ${ARCHIVE}\$" "${TMP_DIR}/SHA256SUMS.txt" | awk '{print $1}')"
if [ -z "$EXPECTED_SHA" ]; then
  echo "error: checksum for ${ARCHIVE} not found in SHA256SUMS.txt" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA="$(sha256sum "${TMP_DIR}/${ARCHIVE}" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL_SHA="$(shasum -a 256 "${TMP_DIR}/${ARCHIVE}" | awk '{print $1}')"
else
  echo "error: neither sha256sum nor shasum found" >&2
  exit 1
fi

if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
  echo "error: checksum mismatch! expected ${EXPECTED_SHA}, got ${ACTUAL_SHA}" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
tar -xzf "${TMP_DIR}/${ARCHIVE}" -C "$TMP_DIR" sub
chmod +x "${TMP_DIR}/sub"
mv "${TMP_DIR}/sub" "${INSTALL_DIR}/sub"

echo "Installed sub to ${INSTALL_DIR}/sub"
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *) echo "Add ${INSTALL_DIR} to your PATH to run 'sub' directly." ;;
esac
