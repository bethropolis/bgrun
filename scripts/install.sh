#!/usr/bin/env sh
set -eu

REPO="bethropolis/bgrun"
INSTALL_DIR="${HOME}/.local/bin"
MAX_RETRIES=3
RETRY_DELAY=5

case "$(uname -m)" in
  x86_64)  ARCH="amd64" ;;
  aarch64) ARCH="arm64" ;;
  *)
    echo "Unsupported architecture: $(uname -m)"
    exit 1
    ;;
esac

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"

if [ "$OS" != "linux" ]; then
  echo "This install script is intended for Linux."
  exit 1
fi

fetch_latest_url() {
  curl -s "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep "browser_download_url.*${OS}_${ARCH}" \
    | head -1 \
    | cut -d '"' -f 4
}

echo "==> Fetching latest release for ${OS}/${ARCH}..."

DOWNLOAD_URL=""
for i in 1 2 3; do
  DOWNLOAD_URL=$(fetch_latest_url) || true
  if [ -n "$DOWNLOAD_URL" ]; then
    break
  fi
  # Check for rate limiting
  RATE_LIMIT=$(curl -s -o /dev/null -w "%{http_code}" "https://api.github.com/rate_limit" 2>/dev/null || echo "200")
  if [ "$RATE_LIMIT" = "403" ]; then
    echo "==> GitHub API rate limited. Waiting ${RETRY_DELAY}s before retry (${i}/${MAX_RETRIES})..."
  else
    echo "==> Release assets not ready yet. Waiting ${RETRY_DELAY}s before retry (${i}/${MAX_RETRIES})..."
  fi
  sleep "$RETRY_DELAY"
done

if [ -z "$DOWNLOAD_URL" ]; then
  echo "No release found for ${OS}/${ARCH} after ${MAX_RETRIES} attempts."
  echo "You can manually download from: https://github.com/${REPO}/releases/latest"
  exit 1
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "==> Downloading bgrun..."
curl -sSL "$DOWNLOAD_URL" | tar -xz -C "$TMPDIR"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$TMPDIR/bgrun" "$INSTALL_DIR/bgrun"
install -m 0755 "$TMPDIR/bgrun-daemon" "$INSTALL_DIR/bgrun-daemon"

echo "==> Installed to ${INSTALL_DIR}"

if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
  echo "==> ${INSTALL_DIR} is not in PATH."
  echo "    Add 'export PATH=\"${INSTALL_DIR}:\$PATH\"' to your shell profile."
fi

echo "==> Done."
