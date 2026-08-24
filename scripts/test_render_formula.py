#!/usr/bin/env python3
"""Smoke test for scripts/render_formula.py."""

import os
import subprocess
import sys

env = dict(
    os.environ,
    VERSION="0.1.0",
    SHA_MAC_ARM="a" * 64,
    SHA_MAC_X64="b" * 64,
    SHA_LINUX_X64="c" * 64,
    SHA_LINUX_ARM="d" * 64,
)

out = subprocess.run(
    [sys.executable, "scripts/render_formula.py"],
    env=env,
    capture_output=True,
    text=True,
)
assert out.returncode == 0, out.stderr
formula = out.stdout

# Version and all four checksums are substituted.
assert 'version "0.1.0"' in formula, formula
assert "a" * 64 in formula and "b" * 64 in formula, formula
assert "c" * 64 in formula and "d" * 64 in formula, formula

# Ruby interpolations survive (they are runtime, not build-time).
assert '#{bin}' in formula, formula

# All four release assets referenced.
for target in [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
]:
    assert f"cue-{target}.tar.gz" in formula, target

# Missing environment is a clean failure.
sparse = {k: v for k, v in env.items() if k != "SHA_LINUX_ARM"}
missing = subprocess.run(
    [sys.executable, "scripts/render_formula.py"],
    env=sparse,
    capture_output=True,
    text=True,
)
assert missing.returncode == 1
assert "SHA_LINUX_ARM" in missing.stderr

print("formula renderer ok")
