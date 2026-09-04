#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "This install script is intended for Linux."
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required (Rust toolchain). Install Rust from https://rustup.rs/"
  exit 1
fi

# version_ge HAVE NEED — true if HAVE >= NEED (numeric, dot-separated)
version_ge() {
  local IFS=.
  local -a have need
  read -ra have <<< "$1"
  read -ra need <<< "$2"
  local i h n
  for ((i = 0; i < ${#need[@]}; i++)); do
    h="${have[i]:-0}"
    n="${need[i]:-0}"
    h="${h%%[^0-9]*}"
    n="${n%%[^0-9]*}"
    if ((10#$h > 10#$n)); then return 0; fi
    if ((10#$h < 10#$n)); then return 1; fi
  done
  return 0
}

# The repo pins a toolchain via rust-toolchain.toml. If that exact toolchain
# is broken or missing locally, fall back to any installed toolchain >= MSRV.
# NOTE: probe with `rustc`, not `cargo --version` — cargo prints its own
# version without ever invoking rustc, so it passes even on a broken toolchain.
ensure_working_toolchain() {
  if rustc --version >/dev/null 2>&1; then
    return 0
  fi
  if ! command -v rustup >/dev/null 2>&1; then
    echo "cargo is not working and rustup is unavailable; reinstall Rust from https://rustup.rs/"
    exit 1
  fi
  local msrv
  msrv="$(sed -n 's/^rust-version *= *"//p' Cargo.toml 2>/dev/null | head -n1 | cut -d'"' -f1 || true)"
  [[ -z "$msrv" ]] && msrv="1.90"
  echo "Pinned toolchain unavailable; looking for any installed toolchain >= ${msrv}..."
  local best_toolchain="" best_version="0.0" name ver
  while IFS= read -r line; do
    name="${line%% *}"
    [[ -z "$name" ]] && continue
    # NOTE: `|| true` — a broken toolchain makes this pipeline fail, and
    # `set -e`/`pipefail` would otherwise kill the whole installer here.
    ver="$(rustup run "$name" rustc --version 2>/dev/null | awk '{print $2}' || true)"
    [[ -z "$ver" ]] && continue
    if version_ge "$ver" "$msrv" && version_ge "$ver" "$best_version"; then
      best_version="$ver"
      best_toolchain="$name"
    fi
  done < <(rustup toolchain list 2>/dev/null)
  if [[ -z "$best_toolchain" ]]; then
    echo "No working Rust toolchain >= ${msrv} found. Try: rustup toolchain install stable"
    exit 1
  fi
  echo "Using toolchain ${best_toolchain} (rustc ${best_version})"
  export RUSTUP_TOOLCHAIN="$best_toolchain"
}

install_skill=false
print_skill=false

usage() {
  cat <<'USAGE'
Usage: ./install.sh [--install-skill] [--print-skill]

  --install-skill  Copy docs/bgrun skill to ~/.config/opencode/skills/<skill name>/
  --print-skill    Print docs/bgrun/SKILL.md to stdout and exit unless combined
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-skill)
      install_skill=true
      shift
      ;;
    --print-skill)
      print_skill=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1"
      usage
      exit 1
      ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$repo_root"

skill_src_dir="${repo_root}/docs/bgrun"
skill_file="${skill_src_dir}/SKILL.md"

if [[ ! -f "$skill_file" ]]; then
  echo "Skill file not found at ${skill_file}"
  exit 1
fi

if $print_skill && ! $install_skill; then
  cat "$skill_file"
  exit 0
fi

skill_name="$(awk -F': ' '/^name: /{print $2; exit}' "$skill_file")"
if [[ -z "$skill_name" ]]; then
  skill_name="bgrun"
fi

color_primary=$'\033[38;2;0;214;255m'
color_secondary=$'\033[38;2;160;100;255m'
color_reset=$'\033[0m'

banner=$(cat <<'BANNER'
 ███████████    █████████  ███████████   █████  █████ ██████   █████
▒▒███▒▒▒▒▒███  ███▒▒▒▒▒███▒▒███▒▒▒▒▒███ ▒▒███  ▒▒███ ▒▒██████ ▒▒███ 
 ▒███    ▒███ ███     ▒▒▒  ▒███    ▒███  ▒███   ▒███  ▒███▒███ ▒███ 
 ▒██████████ ▒███          ▒██████████   ▒███   ▒███  ▒███▒▒███▒███ 
 ▒███▒▒▒▒▒███▒███    █████ ▒███▒▒▒▒▒███  ▒███   ▒███  ▒███ ▒▒██████ 
 ▒███    ▒███▒▒███  ▒▒███  ▒███    ▒███  ▒███   ▒███  ▒███  ▒▒█████ 
 ███████████  ▒▒█████████  █████   █████ ▒▒████████   █████  ▒▒█████
▒▒▒▒▒▒▒▒▒▒▒    ▒▒▒▒▒▒▒▒▒  ▒▒▒▒▒   ▒▒▒▒▒   ▒▒▒▒▒▒▒▒   ▒▒▒▒▒    ▒▒▒▒▒ 
BANNER
)

echo
while IFS= read -r line; do
  line=${line//█/${color_primary}█${color_reset}}
  line=${line//▒/${color_secondary}▒${color_reset}}
  printf '%b\n' "$line"
done <<< "$banner"
echo
echo

echo "==> Building bgrun (release)..."
ensure_working_toolchain
cargo build --release -p bgrun-cli -p bgrun-daemon

install_dir="${HOME}/.local/bin"
mkdir -p "$install_dir"

echo "==> Installing binaries to ${install_dir}..."
install -m 0755 target/release/bgrun "$install_dir/bgrun"
install -m 0755 target/release/bgrun-daemon "$install_dir/bgrun-daemon"

if $install_skill; then
  if command -v opencode >/dev/null 2>&1 || [[ -d "${HOME}/.config/opencode" ]]; then
    skills_dir="${HOME}/.config/opencode/skills"
    skill_target="${skills_dir}/${skill_name}"
    echo "==> Installing skill to ${skill_target}..."
    mkdir -p "$skill_target"
    cp -a "${skill_src_dir}/." "$skill_target/"
  else
    echo "==> OpenCode not detected; skipping skill install."
  fi
fi

echo "==> Installing shell completions..."
completions_src="${repo_root}/packaging/completions"

# Fish
if command -v fish >/dev/null 2>&1; then
  fish_completions_dir="${HOME}/.config/fish/completions"
  mkdir -p "$fish_completions_dir"
  install -m 0644 "${completions_src}/bgrun.fish" "${fish_completions_dir}/bgrun.fish"
  echo "  fish:   ${fish_completions_dir}/bgrun.fish"
fi

# Bash (user-local, compatible with bash-completion@2)
if command -v bash >/dev/null 2>&1; then
  bash_completions_dir="${HOME}/.local/share/bash-completion/completions"
  mkdir -p "$bash_completions_dir"
  install -m 0644 "${completions_src}/bgrun.bash" "${bash_completions_dir}/bgrun"
  echo "  bash:   ${bash_completions_dir}/bgrun"
fi

# Zsh
if command -v zsh >/dev/null 2>&1; then
  zsh_completions_dir="${HOME}/.zsh/completions"
  mkdir -p "$zsh_completions_dir"
  install -m 0644 "${completions_src}/bgrun.zsh" "${zsh_completions_dir}/_bgrun"
  echo "  zsh:    ${zsh_completions_dir}/_bgrun"
  # Ensure ~/.zsh/completions is in fpath
  if ! grep -qF "${zsh_completions_dir}" "${HOME}/.zshrc" 2>/dev/null; then
    echo "  zsh:    Add 'fpath+=(\"${zsh_completions_dir}\")' to ~/.zshrc and run 'compinit'"
  fi
fi

if [[ ":$PATH:" != *":${install_dir}:"* ]]; then
  echo
  echo "==> PATH update needed"
  echo "${install_dir} is not in your PATH."
  echo "Add this to your shell profile:"
  echo "  export PATH=\"${install_dir}:\$PATH\""
fi

echo
echo "==> Done! Installed bgrun to ${install_dir}"
