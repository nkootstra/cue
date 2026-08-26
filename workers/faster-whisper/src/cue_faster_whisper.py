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


def diagnostic(message: str) -> None:
    lines = str(message).splitlines() or ["unknown worker failure"]
    for line in lines:
        print(f"cue-faster-whisper: {line}", file=sys.stderr)


class WorkerArgumentParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        diagnostic(f"invalid arguments: {message}")
        raise SystemExit(2)


def emit(payload: dict) -> None:
    encoded = json.dumps(payload, ensure_ascii=False)
    sys.stdout.write(f"{encoded}\n")


def load_backend():
    import faster_whisper

    return faster_whisper


def check(faster_whisper) -> int:
    emit({"ok": True, "faster_whisper": faster_whisper.__version__})
    return 0


def transcribe(
    faster_whisper, input_path: str, model_name: str, language: str | None
) -> int:
    if language == "auto":
        language = None

    diagnostic(f"loading model {model_name}")
    model = faster_whisper.WhisperModel(
        model_name, device="auto", compute_type="auto"
    )
    segments_iter, info = model.transcribe(
        input_path,
        language=language,
        word_timestamps=True,
        vad_filter=True,
        vad_parameters={"min_silence_duration_ms": 300},
    )
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
        # PROGRESS lines; anything else here is a prefixed human log.
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


def run(args: argparse.Namespace) -> int:
    faster_whisper = load_backend()
    if args.check:
        return check(faster_whisper)
    return transcribe(faster_whisper, args.input, args.model, args.language)


def main() -> int:
    parser = WorkerArgumentParser(prog="cue-faster-whisper")
    parser.add_argument("--input", help="audio file to transcribe")
    parser.add_argument("--model", default="large-v3-turbo")
    parser.add_argument("--language", default=None)
    parser.add_argument("--check", action="store_true", help="verify imports and exit")
    args = parser.parse_args()

    if not args.input:
        if not args.check:
            parser.error("--input is required unless --check is given")

    try:
        return run(args)
    except Exception as exc:
        diagnostic(exc)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
