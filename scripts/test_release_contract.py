#!/usr/bin/env python3
"""Offline tests for the release delivery contract."""

from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / "scripts" / "release_contract.py"
TARGETS = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
]


def workspace_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as manifest:
        return tomllib.load(manifest)["workspace"]["package"]["version"]


def run(*args: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(CONTRACT), *(str(arg) for arg in args)],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )


def checksum_lines() -> list[str]:
    return [
        f"{str(index + 1) * 64}  cue-{target}.tar.gz"
        for index, target in enumerate(TARGETS)
    ]


with tempfile.TemporaryDirectory() as directory:
    workspace = Path(directory)
    version = workspace_version()
    matching_tag = f"v{version}"
    mismatched_tag = f"v{version}.mismatch"
    checksums = workspace / "sha256sums.txt"
    formula = workspace / "cue.rb"
    checksums.write_text("\n".join(checksum_lines()) + "\n", encoding="utf-8")

    # A matching v-prefixed tag is accepted; a mismatched tag stops publishing.
    matching = run(
        "check-version",
        "--tag",
        matching_tag,
        "--manifest",
        ROOT / "Cargo.toml",
    )
    assert matching.returncode == 0, matching.stderr
    mismatched = run(
        "check-version",
        "--tag",
        mismatched_tag,
        "--manifest",
        ROOT / "Cargo.toml",
    )
    assert mismatched.returncode != 0
    assert "does not match workspace version" in mismatched.stderr

    # Rendering works in a clean workspace that has no Formula/ directory.
    assert not (workspace / "Formula").exists()
    rendered = run(
        "render-formula",
        "--tag",
        matching_tag,
        "--checksums",
        checksums,
        "--output",
        formula,
    )
    assert rendered.returncode == 0, rendered.stderr
    text = formula.read_text(encoding="utf-8")
    assert f'version "{version}"' in text
    for index, target in enumerate(TARGETS):
        assert f"cue-{target}.tar.gz" in text
        assert str(index + 1) * 64 in text

    invalid_cases = {
        "missing": checksum_lines()[:-1],
        "duplicate": [*checksum_lines(), checksum_lines()[0]],
        "malformed": [*checksum_lines()[:-1], "not-a-checksum"],
    }
    for expected, lines in invalid_cases.items():
        checksums.write_text("\n".join(lines) + "\n", encoding="utf-8")
        invalid = run(
            "render-formula",
            "--tag",
            matching_tag,
            "--checksums",
            checksums,
            "--output",
            formula,
        )
        assert invalid.returncode != 0, expected
        assert expected in invalid.stderr.lower(), invalid.stderr

print("release contract ok")
