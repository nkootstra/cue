#!/usr/bin/env python3
"""Grade a transcribe-skill eval workspace without modifying it."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys


SUPPORTED_CHECKS = {
    "correction_receipt",
    "file_exists",
    "file_nonempty",
    "contains",
    "not_contains",
    "files_equal",
}


def is_hash(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def is_correction_receipt(path: Path) -> bool:
    try:
        receipt = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return False
    if (
        not isinstance(receipt, dict)
        or type(receipt.get("schema_version")) is not int
        or receipt["schema_version"] != 1
    ):
        return False
    if not is_hash(receipt.get("manifest_hash")):
        return False
    if receipt.get("manifest_source") not in {
        "explicit",
        "output-directory",
        "parent-directory",
    }:
        return False
    source_hashes = receipt.get("source_hashes")
    if (
        not isinstance(source_hashes, dict)
        or "normalized" not in source_hashes
        or not is_hash(source_hashes.get("transcript"))
    ):
        return False
    normalized_hash = source_hashes.get("normalized")
    if normalized_hash is not None and not is_hash(normalized_hash):
        return False
    rules = receipt.get("rules")
    if not isinstance(rules, list) or not rules:
        return False
    for rule in rules:
        if not isinstance(rule, dict):
            return False
        if not isinstance(rule.get("find"), str) or not rule["find"]:
            return False
        if not isinstance(rule.get("replace"), str):
            return False
        applications = rule.get("applications")
        if not isinstance(applications, list) or not applications:
            return False
        if any(
            not isinstance(application, dict)
            or not isinstance(application.get("artifact"), str)
            or not application["artifact"]
            or not isinstance(application.get("replacements"), int)
            or isinstance(application.get("replacements"), bool)
            or application["replacements"] < 0
            for application in applications
        ):
            return False
    return True


def load_checks(rubric_path: Path) -> list[dict[str, object]]:
    rubric = json.loads(rubric_path.read_text(encoding="utf-8"))
    if not isinstance(rubric, dict):
        raise ValueError("rubric must be an object")
    evals = rubric.get("evals")
    if not isinstance(evals, list) or not evals:
        raise ValueError("rubric must contain a non-empty evals list")

    checks: list[dict[str, object]] = []
    ids: set[int] = set()
    for evaluation in evals:
        if not isinstance(evaluation, dict):
            raise ValueError("every eval must be an object")
        eval_id = evaluation.get("id")
        if not isinstance(eval_id, int) or eval_id in ids:
            raise ValueError("every eval must have a unique integer id")
        ids.add(eval_id)
        eval_checks = evaluation.get("assertions")
        if not isinstance(eval_checks, list) or not eval_checks:
            raise ValueError(f"eval {eval_id} must contain at least one structured check")
        for check in eval_checks:
            if not isinstance(check, dict):
                raise ValueError(f"eval {eval_id} contains a non-object check")
            check_type = check.get("type")
            if check_type not in SUPPORTED_CHECKS:
                raise ValueError(f"eval {eval_id} has unsupported check type: {check_type}")
            if not isinstance(check.get("name"), str) or not isinstance(check.get("path"), str):
                raise ValueError(f"eval {eval_id} check needs string name and path")
            if check_type in {"contains", "not_contains"} and not isinstance(
                check.get("value"), str
            ):
                raise ValueError(f"eval {eval_id} {check_type} check needs a string value")
            if check_type == "files_equal" and not isinstance(check.get("other_path"), str):
                raise ValueError(f"eval {eval_id} files_equal check needs other_path")
            checks.append({**check, "eval_id": eval_id})
    return checks


def resolve(workspace: Path, relative: object) -> Path:
    candidate = (workspace / str(relative)).resolve()
    try:
        candidate.relative_to(workspace)
    except ValueError as error:
        raise ValueError(f"rubric path escapes workspace: {relative}") from error
    return candidate


def evaluate(check: dict[str, object], workspace: Path) -> bool:
    path = resolve(workspace, check["path"])
    check_type = check["type"]
    if check_type == "file_exists":
        return path.is_file()
    if check_type == "correction_receipt":
        return path.is_file() and is_correction_receipt(path)
    if check_type == "file_nonempty":
        return path.is_file() and path.stat().st_size > 0
    if check_type == "files_equal":
        other = resolve(workspace, check["other_path"])
        return path.is_file() and other.is_file() and path.read_bytes() == other.read_bytes()
    if not path.is_file():
        return False
    content = path.read_text(encoding="utf-8", errors="replace")
    expected = str(check["value"])
    if check.get("case_insensitive", False):
        content, expected = content.casefold(), expected.casefold()
    return (expected in content) == (check_type == "contains")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rubric", required=True, type=Path)
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--validate-rubric", action="store_true")
    args = parser.parse_args()

    try:
        checks = load_checks(args.rubric)
        if args.validate_rubric:
            print(f"rubric ok: {len(checks)} checks")
            return 0
        if args.workspace is None:
            parser.error("--workspace is required unless --validate-rubric is used")
        workspace = args.workspace.resolve()
        if not workspace.is_dir():
            raise ValueError(f"workspace does not exist: {workspace}")

        passed = 0
        for check in checks:
            ok = evaluate(check, workspace)
            passed += int(ok)
            status = "PASS" if ok else "FAIL"
            print(f"{status}  eval {check['eval_id']}: {check['name']}")
        failed = len(checks) - passed
        print(f"TOTAL: {passed} passed, {failed} failed")
        return int(failed != 0)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"grade error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
