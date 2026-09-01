#!/usr/bin/env sh
set -eu

REPO="bethropolis/bgrun"
INSTALL_DIR="${HOME}/.local/bin"
MAX_RETRIES=3
RETRY_DELAY=5

# ── colors & symbols (auto-disable if not a tty) ────────────────────────────
if [ -t 1 ] && [ "${TERM:-dumb}" != "dumb" ] && [ "${NO_COLOR:-}" = "" ]; then
  BOLD="\033[1m"; DIM="\033[2m"; RESET="\033[0m"
  GREEN="\033[32m"; CYAN="\033[36m"; YELLOW="\033[33m"; RED="\033[31m"; GREY="\033[90m"
else
  BOLD=""; DIM=""; RESET=""; GREEN=""; CYAN=""; YELLOW=""; RED=""; GREY=""
fi

if [ -t 1 ] && command -v locale >/dev/null 2>&1 && locale charmap 2>/dev/null | grep -qi "UTF-8"; then
  OK="✓"; ARROW="→"; DOT="●"; WARN_SYM="⚠"; CROSS="✗"
else
  OK="[ok]"; ARROW="->"; DOT="*"; WARN_SYM="!"; CROSS="[x]"
fi

info()  { printf "  ${CYAN}${DOT}${RESET} %s\n" "$*"; }
ok()    { printf "  ${GREEN}${OK}${RESET} %s\n" "$*"; }
warn()  { printf "  ${YELLOW}${WARN_SYM}${RESET} %s\n" "$*"; }
err()   { printf "  ${RED}${CROSS}${RESET} %s\n" "$*" >&2; }
step()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }
dim()   { printf "${GREY}%s${RESET}\n" "$*"; }

# ── banner ──────────────────────────────────────────────────────────────────
printf "\n"
printf "${BOLD}  ██████╗  ██████╗ ██████╗ ██╗   ██╗███╗   ██╗${RESET}\n"
printf "${BOLD}  ██╔══██╗██╔════╝ ██╔══██╗██║   ██║████╗  ██║${RESET}\n"
printf "${BOLD}  ██████╔╝██║  ███╗██████╔╝██║   ██║██╔██╗ ██║${RESET}\n"
printf "${BOLD}  ██╔══██╗██║   ██║██╔══██╗██║   ██║██║╚██╗██║${RESET}\n"
printf "${BOLD}  ██████╔╝╚██████╔╝██║  ██║╚██████╔╝██║ ╚████║${RESET}\n"
printf "${BOLD}  ╚═════╝  ╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝${RESET}\n"
printf "${GREY}         background process runner ─ https://github.com/${REPO}${RESET}\n"
printf "\n"

# ── platform ────────────────────────────────────────────────────────────────
case "$(uname -m)" in
  x86_64)  ARCH="amd64" ;;
  aarch64|arm64) ARCH="arm64" ;;
  *)
    err "Unsupported architecture: $(uname -m)"
    dim "  bgrun currently supports x86_64 and aarch64."
    exit 1
    ;;
esac

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"

if [ "$OS" != "linux" ]; then
  err "This installer is for Linux."
  dim "  Detected OS: $OS — see https://github.com/${REPO}/releases for alternatives."
  exit 1
fi

PLATFORM="${OS}/${ARCH}"
info "Platform  ${BOLD}${PLATFORM}${RESET} ${DIM}($(uname -m))${RESET}"

# ── existing install ────────────────────────────────────────────────────────
if command -v bgrun >/dev/null 2>&1; then
  EXISTING="$(bgrun --version 2>/dev/null || echo "unknown")"
  dim "  Found existing install: ${EXISTING} at $(command -v bgrun)"
fi

# ── helpers ─────────────────────────────────────────────────────────────────
fetch_latest_url() {
  curl -s --max-time 10 "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null \
    | grep "browser_download_url.*${OS}_${ARCH}" \
    | head -1 \
    | cut -d '"' -f 4 || true
}

fetch_latest_tag() {
  curl -s --max-time 10 "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null \
    | grep '"tag_name"' \
    | head -1 \
    | cut -d '"' -f 4 || true
}

# ── fetch release ───────────────────────────────────────────────────────────
step "Fetching latest release"

DOWNLOAD_URL=""
LATEST_TAG=""
for i in 1 2 3; do
  if [ -t 1 ]; then
    printf "  ${CYAN}${DOT}${RESET} Contacting GitHub API ${GREY}(attempt ${i}/${MAX_RETRIES})${RESET}…"
  fi

  DOWNLOAD_URL=$(fetch_latest_url)
  LATEST_TAG=$(fetch_latest_tag)

  if [ -n "$DOWNLOAD_URL" ]; then
    if [ -t 1 ]; then printf " ${GREEN}${OK}${RESET}\n"; fi
    break
  fi

  if [ -t 1 ]; then printf " ${YELLOW}retrying…${RESET}\n"; fi

  if [ "$i" -lt "$MAX_RETRIES" ]; then
    dim "  Release not ready or rate-limited — waiting ${RETRY_DELAY}s…"
    sleep "$RETRY_DELAY"
  fi
done

if [ -z "$DOWNLOAD_URL" ]; then
  err "No release found for ${PLATFORM} after ${MAX_RETRIES} attempts."
  printf "\n"
  dim "  Download manually:"
  dim "  ${ARROW} https://github.com/${REPO}/releases/latest"
  printf "\n"
  exit 1
fi

if [ -n "$LATEST_TAG" ]; then
  ok "Latest: ${BOLD}${LATEST_TAG}${RESET}"
else
  ok "Release found"
fi
dim "  ${DOWNLOAD_URL}"

# ── download & install ──────────────────────────────────────────────────────
step "Installing"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

info "Downloading tarball…"
curl -sSL "$DOWNLOAD_URL" | tar -xz -C "$TMPDIR"
ok "Downloaded"

info "Installing to ${BOLD}${INSTALL_DIR}${RESET}…"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$TMPDIR/bgrun" "$INSTALL_DIR/bgrun"
install -m 0755 "$TMPDIR/bgrun-daemon" "$INSTALL_DIR/bgrun-daemon"
ok "Installed ${BOLD}bgrun${RESET} ${DIM}${LATEST_TAG:-}${RESET} ${ARROW} ${INSTALL_DIR}/bgrun"
ok "Installed ${BOLD}bgrun-daemon${RESET} ${ARROW} ${INSTALL_DIR}/bgrun-daemon"

# ── verify ──────────────────────────────────────────────────────────────────
if [ -x "$INSTALL_DIR/bgrun" ]; then
  INSTALLED_VER="$("$INSTALL_DIR/bgrun" --version 2>/dev/null || echo "")"
  if [ -n "$INSTALLED_VER" ]; then
    dim "  ${INSTALLED_VER}"
  fi
fi

# ── PATH hint ───────────────────────────────────────────────────────────────
printf "\n"
if ! echo ":$PATH:" | grep -q ":$INSTALL_DIR:"; then
  warn "${INSTALL_DIR} is not in PATH"
  printf "\n"
  printf "  ${DIM}Add it to your shell profile:${RESET}\n"
  printf "  ${BOLD}  echo 'export PATH=\"%s:\$PATH\"' >> ~/.bashrc && source ~/.bashrc${RESET}\n" "$INSTALL_DIR"
  printf "  ${GREY}  (or ~/.zshrc for zsh, ~/.config/fish/config.fish for fish)${RESET}\n"
else
  ok "PATH looks good"
fi

# ── done ────────────────────────────────────────────────────────────────────
printf "\n"
printf "${GREEN}${BOLD}  ${OK} Done!${RESET} ${DIM}— run ${RESET}${BOLD}bgrun --help${RESET}${DIM} to get started.${RESET}\n"
printf "\n"
