#!/usr/bin/env python3
"""cue's faster-whisper transcription worker.

Contract with the Rust adapter:

- stdout carries machine-readable JSON only
- stderr carries human-readable diagnostics
- exit code 0 means the JSON on stdout is valid output

Usage:
    cue-faster-whisper --input AUDIO --model MODEL [--language LANG]
    cue-faster-whisper --check

Output schema (version 1):
{
  "version": 1,
  "language": "en",
  "duration": 61.5,
  "segments": [
    {
      "id": 0,
      "start": 0.0, "end": 2.4, "text": " hello world.",
      "words": [{"word": " hello", "start": 0.0, "end": 0.4,
                 "probability": 0.97}]
    }
  ]
}
"""

from __future__ import annotations

import argparse
import json
import sys


WORKER_VERSION = 1


def fail(message: str) -> None:
    print(f"cue-faster-whisper: {message}", file=sys.stderr)
    raise SystemExit(1)


def emit(payload: dict) -> None:
    json.dump(payload, sys.stdout, ensure_ascii=False)
    sys.stdout.write("\n")


def check() -> int:
    try:
        import faster_whisper  # noqa: F401
    except Exception as exc:  # pragma: no cover - depends on environment
        fail(f"faster-whisper is not importable: {exc}")
        return 1
    import faster_whisper

    emit({"ok": True, "faster_whisper": faster_whisper.__version__})
    return 0


def transcribe(input_path: str, model_name: str, language: str | None) -> int:
    if language == "auto":
        language = None

    try:
        from faster_whisper import WhisperModel
    except Exception as exc:  # pragma: no cover - depends on environment
        fail(f"could not import faster_whisper (is the venv provisioned?): {exc}")
        return 1

    print(f"loading model {model_name}", file=sys.stderr)
    try:
        model = WhisperModel(model_name, device="auto", compute_type="auto")
        segments_iter, info = model.transcribe(
            input_path,
            language=language,
            word_timestamps=True,
            vad_filter=True,
            vad_parameters={"min_silence_duration_ms": 300},
        )
    except Exception as exc:
        fail(f"transcription failed for {input_path}: {exc}")
        return 1

    segments = []
    for index, segment in enumerate(segments_iter):
        words = []
        for word in segment.words or []:
            words.append(
                {
                    "word": word.word,
                    "start": word.start,
                    "end": word.end,
                    "probability": round(word.probability, 4),
                }
            )
        segments.append(
            {
                "id": index,
                "start": segment.start,
                "end": segment.end,
                "text": segment.text,
                "words": words,
            }
        )
        # Machine-readable progress on stderr: the Rust adapter parses
        # PROGRESS lines; anything else here is a human log.
        if info.duration and info.duration > 0:
            fraction = min(segment.end / info.duration, 1.0)
            print(f"PROGRESS {fraction:.3f}", file=sys.stderr, flush=True)

    emit(
        {
            "version": WORKER_VERSION,
            "language": info.language,
            "duration": info.duration,
            "segments": segments,
        }
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(prog="cue-faster-whisper")
    parser.add_argument("--input", help="audio file to transcribe")
    parser.add_argument("--model", default="large-v3-turbo")
    parser.add_argument("--language", default=None)
    parser.add_argument("--check", action="store_true", help="verify imports and exit")
    args = parser.parse_args()

    if args.check:
        return check()
    if not args.input:
        parser.error("--input is required unless --check is given")

    return transcribe(args.input, args.model, args.language)


if __name__ == "__main__":
    raise SystemExit(main())
