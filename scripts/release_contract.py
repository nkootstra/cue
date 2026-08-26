#!/usr/bin/env python3
"""Validate and render the inputs shared by the release workflow jobs."""

import argparse
from pathlib import Path
import re
import sys
import tomllib

from render_formula import render_formula


TARGET_ENVIRONMENT = {
    "aarch64-apple-darwin": "SHA_MAC_ARM",
    "x86_64-apple-darwin": "SHA_MAC_X64",
    "x86_64-unknown-linux-gnu": "SHA_LINUX_X64",
    "aarch64-unknown-linux-gnu": "SHA_LINUX_ARM",
}
CHECKSUM_LINE = re.compile(r"^([0-9a-fA-F]{64})[ \t]+\*?(\S+)$")


def release_version(tag: str) -> str:
    version = tag.removeprefix("v")
    if not version:
        raise ValueError("release tag has no version")
    return version


def workspace_version(manifest: Path) -> str:
    with manifest.open("rb") as file:
        document = tomllib.load(file)
    try:
        version = document["workspace"]["package"]["version"]
    except (KeyError, TypeError) as error:
        raise ValueError(f"{manifest} has no workspace.package.version") from error
    if not isinstance(version, str) or not version:
        raise ValueError(f"{manifest} has an invalid workspace.package.version")
    return version


def validate_version(tag: str, manifest: Path) -> str:
    version = release_version(tag)
    expected = workspace_version(manifest)
    if version != expected:
        raise ValueError(
            f"release tag {tag!r} does not match workspace version {expected!r}"
        )
    return version


def read_checksums(path: Path) -> dict[str, str]:
    expected_files = {
        f"cue-{target}.tar.gz": environment
        for target, environment in TARGET_ENVIRONMENT.items()
    }
    checksums: dict[str, str] = {}

    for line_number, raw_line in enumerate(
        path.read_text(encoding="utf-8").splitlines(), start=1
    ):
        match = CHECKSUM_LINE.fullmatch(raw_line)
        if match is None:
            raise ValueError(f"malformed checksum on line {line_number}")
        checksum, filename = match.groups()
        if filename not in expected_files:
            raise ValueError(f"unexpected checksum entry {filename!r}")
        if filename in checksums:
            raise ValueError(f"duplicate checksum entry {filename!r}")
        checksums[filename] = checksum.lower()

    missing = sorted(set(expected_files) - set(checksums))
    if missing:
        raise ValueError(f"missing checksum entries: {', '.join(missing)}")

    return {
        environment: checksums[filename]
        for filename, environment in expected_files.items()
    }


def check_version_command(arguments: argparse.Namespace) -> None:
    validate_version(arguments.tag, arguments.manifest)


def render_formula_command(arguments: argparse.Namespace) -> None:
    version = release_version(arguments.tag)
    values = {"VERSION": version, **read_checksums(arguments.checksums)}
    output = render_formula(values)
    if not arguments.output.parent.is_dir():
        raise ValueError(f"output directory does not exist: {arguments.output.parent}")
    arguments.output.write_text(output, encoding="utf-8")


def parser() -> argparse.ArgumentParser:
    command_parser = argparse.ArgumentParser(description=__doc__)
    subcommands = command_parser.add_subparsers(required=True)

    check_version = subcommands.add_parser("check-version")
    check_version.add_argument("--tag", required=True)
    check_version.add_argument("--manifest", type=Path, required=True)
    check_version.set_defaults(action=check_version_command)

    render = subcommands.add_parser("render-formula")
    render.add_argument("--tag", required=True)
    render.add_argument("--checksums", type=Path, required=True)
    render.add_argument("--output", type=Path, required=True)
    render.set_defaults(action=render_formula_command)
    return command_parser


def main() -> int:
    arguments = parser().parse_args()
    try:
        arguments.action(arguments)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
