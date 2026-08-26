#!/bin/bash
# Exercise install.sh --dry-run for every supported platform mapping
# by shadowing uname on PATH.
set -eu
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHIM_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cue-install-test.XXXXXX")"
trap 'rm -rf "$SHIM_DIR"' EXIT

run_case() {
  local os_name="$1" arch_name="$2" expected="$3"
  cat > "$SHIM_DIR/uname" <<EOF
#!/bin/sh
if [ "\$1" = "-s" ]; then echo "$os_name"; else echo "$arch_name"; fi
EOF
  chmod +x "$SHIM_DIR/uname"
  local result
  result=$(PATH="$SHIM_DIR:$PATH" "$ROOT/install.sh" --dry-run)
  echo "$result" | grep -q "platform:  $expected" || {
    echo "FAIL $os_name/$arch_name: expected $expected, got:"; echo "$result"; exit 1
  }
  echo "$result" | grep -q "releases/latest/download/cue-$expected.tar.gz" || {
    echo "FAIL: wrong asset URL for $expected"; exit 1
  }
  echo "PASS  $os_name/$arch_name -> $expected"
}

run_case Darwin arm64 aarch64-apple-darwin
run_case Darwin x86_64 x86_64-apple-darwin
run_case Linux x86_64 x86_64-unknown-linux-gnu
run_case Linux aarch64 aarch64-unknown-linux-gnu

# Unsupported platform fails cleanly.
printf '#!/bin/sh\nif [ "$1" = "-s" ]; then echo SunOS; else echo sparc; fi\n' > "$SHIM_DIR/uname"
chmod +x "$SHIM_DIR/uname"
if PATH="$SHIM_DIR:$PATH" "$ROOT/install.sh" --dry-run > /dev/null 2>&1; then
  echo "FAIL: unsupported platform should error"
  exit 1
fi
echo "PASS  unsupported platform rejected"

# Pinned version changes the URL.
printf '#!/bin/sh\nif [ "$1" = "-s" ]; then echo Darwin; else echo arm64; fi\n' > "$SHIM_DIR/uname"
chmod +x "$SHIM_DIR/uname"
PATH="$SHIM_DIR:$PATH" "$ROOT/install.sh" --version 0.2.0 --dry-run | \
  grep -q "releases/download/v0.2.0/cue-aarch64-apple-darwin.tar.gz"
echo "PASS  pinned version URL"

echo "install.sh platform mapping ok"
