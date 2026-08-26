#!/usr/bin/env bash

set -euo pipefail

cue_bin="${1:?usage: scripts/test_skill_install.sh PATH_TO_CUE}"
test_root="$(mktemp -d)"
install_log="$test_root/install.log"

cleanup() {
  rm -rf "$test_root"
}
trap cleanup EXIT

mkdir -p "$test_root/home" "$test_root/config"

env \
  HOME="$test_root/home" \
  XDG_CONFIG_HOME="$test_root/config" \
  "$cue_bin" skill install --no-telemetry >"$install_log" 2>&1

skill_file="$test_root/home/.agents/skills/transcribe/SKILL.md"
if [[ ! -f "$skill_file" ]]; then
  echo "global skill install did not create $skill_file" >&2
  cat "$install_log" >&2
  exit 1
fi

if grep -q "Failed to install\|does not support global skill installation" "$install_log"; then
  echo "global skill install reported a failure" >&2
  cat "$install_log" >&2
  exit 1
fi
