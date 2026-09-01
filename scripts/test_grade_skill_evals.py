#!/usr/bin/env python3
"""Offline tests for skill evaluation report validation."""

import importlib.util
import json
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("grade_skill_evals", ROOT / "scripts/grade_skill_evals.py")
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


report = {
    "schema_version": 2,
    "output": ".cue/lesson",
    "confidence_below": 0.8,
    "diagnostics": [
        {
            "id": "CUE-REVIEW-SPOKEN-URL",
            "candidate_id": "url-1",
            "observed": "example dot com",
            "proposed": "example.com",
            "word_index": 1,
            "confidence": 0.9,
            "score": 1.0,
        }
    ],
}

with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / "review.json"
    path.write_text(json.dumps(report), encoding="utf-8")
    assert MODULE.is_review_report(path)

print("skill evaluation report validator ok")
