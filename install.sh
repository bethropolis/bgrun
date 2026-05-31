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

if [[ ":$PATH:" != *":${install_dir}:"* ]]; then
  echo
  echo "==> PATH update needed"
  echo "${install_dir} is not in your PATH."
  echo "Add this to your shell profile:"
  echo "  export PATH=\"${install_dir}:\$PATH\""
fi

echo
echo "==> Done! Installed bgrun to ${install_dir}"
