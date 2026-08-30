#!/bin/bash
# Seed and grade the transcribe skill evals.
#
# Usage:
#   scripts/skill-eval-harness.sh --seed [WORKSPACE]
#   scripts/skill-eval-harness.sh --grade WORKSPACE
#   scripts/skill-eval-harness.sh --self-test

set -eu

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUBRIC="$ROOT/skills/transcribe/evals/evals.json"
GRADER="$ROOT/scripts/grade_skill_evals.py"

usage() {
  echo "usage: $0 --seed [WORKSPACE] | --grade WORKSPACE | --self-test" >&2
  exit 2
}

is_empty_dir() {
  [ -d "$1" ] && [ -z "$(find "$1" -mindepth 1 -maxdepth 1 -print -quit)" ]
}

new_workspace() {
  local parent="$ROOT/skills/transcribe-workspace"
  mkdir -p "$parent"
  mktemp -d "$parent/iteration-XXXXXX"
}

seed_workspace() {
  local workspace="$1"
  if [ -e "$workspace" ]; then
    if [ ! -d "$workspace" ] || ! is_empty_dir "$workspace"; then
      echo "refusing to seed non-empty workspace: $workspace" >&2
      return 1
    fi
  else
    mkdir -p "$workspace"
  fi

  command -v say >/dev/null 2>&1 || { echo "seeding requires the macOS 'say' command" >&2; return 1; }
  command -v ffmpeg >/dev/null 2>&1 || { echo "seeding requires ffmpeg" >&2; return 1; }
  [ -x "$ROOT/target/debug/cue" ] || {
    echo "seeding requires target/debug/cue; run 'cargo build' first" >&2
    return 1
  }

  local fixtures="$workspace/fixtures"
  mkdir -p "$fixtures"

  gen_clip() {
    local spoken_text="$1" output="$2"
    say -o "$output.aiff" "$spoken_text"
    ffmpeg -y -v error -i "$output.aiff" -ar 16000 -ac 1 "$output.mp3"
    rm -f "$output.aiff"
  }

  seed_recovery_batch() {
    local output="$1"
    python3 - "$output" <<'PY'
import json
import pathlib
import sys

cwd = pathlib.Path(sys.argv[1]).resolve()
state_root = cwd / ".cue-state"
hash_value = 0xCBF29CE484222325
for byte in str(cwd).encode():
    hash_value ^= byte
    hash_value = (hash_value * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
scope = f"cwd-{hash_value:016x}"
directory = state_root / "batches" / scope
directory.mkdir(parents=True)
source = cwd / "lesson-03.mp4"
record = {
    "schema_version": 1,
    "id": "batch-eval-recovery",
    "cue_version": "0.13.0",
    "created_at_ms": 1,
    "updated_at_ms": 1,
    "cwd": str(cwd),
    "intent": {
        "mode": "transcript-only",
        "language": None,
        "subtitle_formats": [],
        "summary": False,
        "stream": False,
        "corrections": None,
    },
    "items": [{
        "position": 0,
        "source": str(source),
        "workspace": str(cwd / "lesson-03.cue"),
        "published_base": str(cwd / "lesson-03"),
        "state": {
            "status": "failed",
            "attempt": {
                "number": 1,
                "started_at_ms": 1,
                "finished_at_ms": 1,
            },
            "failure": {
                "stage": "transcribe",
                "summary": "prior transcription attempt failed",
                "remedy": "restore the source and run cue resume",
            },
        },
    }],
}
(directory / "batch-eval-recovery.json").write_text(
    json.dumps(record, indent=2) + "\n"
)
PY
  }

  gen_clip \
    "Hi there, I'm John Doe and this is the observability workshop. Welcome to Acme Dev Conf." \
    "$fixtures/clip-01"
  gen_clip \
    "Welcome back, I'm John Doe again. Today we cover spans, metrics, and traces with open telemetry." \
    "$fixtures/clip-02"

  cat > "$fixtures/context.md" <<'EOF'
# Talk / series context

## Speaker
- John Doe (main presenter; placeholder name, spelled D-O-E)

## Platform
- Acme Dev Conf 2025

## Material
- Talk: Intro to Observability

## Terms
- OpenTelemetry (one word, capital O and T)
- traces
- spans
- metrics
- instrumentation
EOF

  local case variant output
  for case in eval-basic-transcribe eval-context-correction eval-recursive-context eval-existing-correction eval-explicit-multi eval-lexicon-flywheel eval-recover-batch; do
    for variant in with_skill without_skill; do
      mkdir -p "$workspace/$case/$variant/outputs"
    done
  done

  # Seed input media only. Agent-produced outputs must not exist before a run.
  for variant in with_skill without_skill; do
    output="$workspace/eval-basic-transcribe/$variant/outputs"
    cp "$fixtures/clip-01.mp3" "$output/"

    output="$workspace/eval-context-correction/$variant/outputs"
    cp "$fixtures/clip-02.mp3" "$fixtures/context.md" "$output/"

    output="$workspace/eval-recursive-context/$variant/outputs"
    mkdir -p "$output/course/module-02"
    cp "$fixtures/clip-01.mp3" "$output/course/"
    cp "$fixtures/clip-02.mp3" "$output/course/module-02/"
    cp "$fixtures/context.md" "$output/"

    # This case intentionally starts with one canonical output to correct.
    output="$workspace/eval-existing-correction/$variant/outputs"
    mkdir -p "$output/1. welcome.cue"
    cp "$fixtures/clip-02.mp3" "$output/1. welcome.mp3"
    "$ROOT/target/debug/cue" "$output/1. welcome.mp3" >/dev/null 2>&1
    printf '\nPRE-EXISTING OUTPUT SENTINEL open telemetry\n' \
      >> "$output/1. welcome.cue/transcript.txt"
    printf '\nPRE-EXISTING OUTPUT SENTINEL open telemetry\n' \
      >> "$output/1. welcome.cue/subtitles.srt"
    mkdir -p "$workspace/.baselines"
    cp "$output/1. welcome.cue/transcript.json" \
      "$workspace/.baselines/$variant-transcript.json"
    cp "$fixtures/clip-01.mp3" "$output/2. what we cover.mp3"
    cp "$fixtures/context.md" "$output/"

    output="$workspace/eval-explicit-multi/$variant/outputs"
    cp "$fixtures/clip-01.mp3" "$fixtures/clip-02.mp3" "$output/"

    output="$workspace/eval-lexicon-flywheel/$variant/outputs"
    mkdir -p "$output/lesson.cue"
    cat > "$output/lesson.cue/transcript.json" <<'EOF'
{
  "schema_version": 1,
  "language": "en",
  "duration_ms": 1000,
  "words": [
    {"text":"open","start_ms":0,"end_ms":300,"confidence":0.4,"speaker":null},
    {"text":"telemetry.","start_ms":310,"end_ms":1000,"confidence":0.9,"speaker":null}
  ],
  "segments": [
    {"start_ms":0,"end_ms":1000,"text":"open telemetry.","word_start":0,"word_end":2}
  ]
}
EOF
    printf 'open telemetry -> OpenTelemetry\n' \
      > "$output/lesson.cue/corrections.md"
    (cd "$output" && "$ROOT/target/debug/cue" correct lesson.cue >/dev/null)
    cp "$output/lesson.cue/transcript.json" \
      "$workspace/.baselines/$variant-flywheel-transcript.json"

    output="$workspace/eval-recover-batch/$variant/outputs"
    seed_recovery_batch "$output"
    cat > "$output/recovery-scenario.md" <<'EOF'
# Recovery scenario

A prior directory batch in this working directory is incomplete. Inspect the
selected recovery record, resume it rather than reconstructing the directory
command, then inspect the batch again. Use the isolated recovery state by first
running `export CUE_STATE_DIR="$PWD/.cue-state"`. The retry still cannot run
because `lesson-03.mp4` remains missing. Save the commands and final report
requested by the eval prompt. Do not edit recovery journals by hand.
EOF
  done

  echo "workspace ready: $workspace"
  print_prompts "$workspace"
}

print_prompts() {
  local workspace="$1"
  cat <<EOF

=== Agent prompts (execute each in your agent) ===

[basic-transcribe] with_skill
  Prompt: Can you transcribe this conference welcome clip for me?
  Input:  $workspace/eval-basic-transcribe/with_skill/outputs/clip-01.mp3
  Save outputs to: $workspace/eval-basic-transcribe/with_skill/outputs

[basic-transcribe] without_skill
  Prompt: Can you transcribe this conference welcome clip for me?
  Input:  $workspace/eval-basic-transcribe/without_skill/outputs/clip-01.mp3
  Save outputs to: $workspace/eval-basic-transcribe/without_skill/outputs

[context-correction] with_skill
  Prompt: Transcribe this conference welcome clip. A context file
  describing the talk is next to the clip — use it to fix any misheard
  names in the transcript.
  Input:  $workspace/eval-context-correction/with_skill/outputs/
  Save outputs to: $workspace/eval-context-correction/with_skill/outputs

[context-correction] without_skill
  Prompt: Transcribe this conference welcome clip.
  Input:  $workspace/eval-context-correction/without_skill/outputs/
  Save outputs to: $workspace/eval-context-correction/without_skill/outputs

[recursive-context] with_skill
  Prompt: A course directory contains a welcome clip at its top level and
  another in a nested module. Transcribe the whole directory recursively,
  then apply the shared context file to both transcripts.
  Input:  $workspace/eval-recursive-context/with_skill/outputs/
  Save outputs to: $workspace/eval-recursive-context/with_skill/outputs

[recursive-context] without_skill
  Prompt: Transcribe every welcome clip under the course directory.
  Input:  $workspace/eval-recursive-context/without_skill/outputs/
  Save outputs to: $workspace/eval-recursive-context/without_skill/outputs

[existing-correction] with_skill
  Prompt: There are two media files, but "1. welcome.mp3" already has a
  pre-existing output folder ("1. welcome.cue") in the current directory.
  Transcribe only the new clip, "2. what we cover.mp3", then write a
  corrections.md manifest for the misheard term and apply it with
  cue correct to the existing folder.
  Input:  $workspace/eval-existing-correction/with_skill/outputs/
  Save outputs to: $workspace/eval-existing-correction/with_skill/outputs

[existing-correction] without_skill
  Prompt: There are two media files and one already-transcribed folder in the
  current directory. Transcribe only the new clip.
  Input:  $workspace/eval-existing-correction/without_skill/outputs/
  Save outputs to: $workspace/eval-existing-correction/without_skill/outputs

[explicit-multi] with_skill
  Prompt: Transcribe clip-01.mp3 and clip-02.mp3 together in one cue command.
  Input:  $workspace/eval-explicit-multi/with_skill/outputs/
  Save outputs to: $workspace/eval-explicit-multi/with_skill/outputs

[explicit-multi] without_skill
  Prompt: Transcribe clip-01.mp3 and clip-02.mp3.
  Input:  $workspace/eval-explicit-multi/without_skill/outputs/
  Save outputs to: $workspace/eval-explicit-multi/without_skill/outputs

[lexicon-flywheel] with_skill
  Prompt: Review lesson.cue with cue and save the JSON report as review.json.
  Then promote the verified "open telemetry" correction into this project
  directory so later lessons reuse it.
  Input:  $workspace/eval-lexicon-flywheel/with_skill/outputs/
  Save outputs to: $workspace/eval-lexicon-flywheel/with_skill/outputs

[lexicon-flywheel] without_skill
  Prompt: Review the existing lesson transcript for likely mistakes.
  Input:  $workspace/eval-lexicon-flywheel/without_skill/outputs/
  Save outputs to: $workspace/eval-lexicon-flywheel/without_skill/outputs

[recover-batch] with_skill
  Prompt: Inspect the incomplete cue batch in this working directory, resume
  it instead of reconstructing the directory command, inspect it again, and
  save the commands in recovery-steps.txt plus the truthful remaining state
  in recovery-report.txt. Follow recovery-scenario.md, including its isolated
  CUE_STATE_DIR setting.
  Input:  $workspace/eval-recover-batch/with_skill/outputs/
  Save outputs to: $workspace/eval-recover-batch/with_skill/outputs

[recover-batch] without_skill
  Prompt: Continue the interrupted media batch and report the result.
  Input:  $workspace/eval-recover-batch/without_skill/outputs/
  Save outputs to: $workspace/eval-recover-batch/without_skill/outputs

=== Then run: scripts/skill-eval-harness.sh --grade $workspace ===
EOF
}

write_synthetic_success() {
  local workspace="$1" output

  write_canonical_transcript() {
    local path="$1"
    cat > "$path" <<'EOF'
{
  "schema_version": 1,
  "language": "en",
  "duration_ms": 1000,
  "words": [
    {"text":"John","start_ms":0,"end_ms":200,"confidence":0.9,"speaker":null},
    {"text":"Doe","start_ms":210,"end_ms":400,"confidence":0.9,"speaker":null},
    {"text":"presents","start_ms":410,"end_ms":600,"confidence":0.9,"speaker":null},
    {"text":"open","start_ms":610,"end_ms":750,"confidence":0.9,"speaker":null},
    {"text":"telemetry.","start_ms":760,"end_ms":1000,"confidence":0.9,"speaker":null}
  ],
  "segments": [
    {"start_ms":0,"end_ms":1000,"text":"John Doe presents open telemetry.","word_start":0,"word_end":5}
  ]
}
EOF
  }

  write_correction_receipt() {
    local path="$1"
    local output_dir manifest
    output_dir="$(dirname "$path")"
    manifest="$output_dir/corrections.md"
    printf 'open telemetry -> OpenTelemetry\n' > "$manifest"
  local manifest_hash transcript_hash
    manifest_hash="$(python3 - "$manifest" "$GRADER" <<'PY'
import importlib.util, pathlib, sys
spec = importlib.util.spec_from_file_location("grader", sys.argv[2])
grader = importlib.util.module_from_spec(spec); spec.loader.exec_module(grader)
print(grader.blake3_hash(pathlib.Path(sys.argv[1]).read_bytes()))
PY
    )"
    transcript_hash="$(python3 - "$output_dir/transcript.json" "$GRADER" <<'PY'
import importlib.util, pathlib, sys
spec = importlib.util.spec_from_file_location("grader", sys.argv[2])
grader = importlib.util.module_from_spec(spec); spec.loader.exec_module(grader)
print(grader.blake3_hash(pathlib.Path(sys.argv[1]).read_bytes()))
PY
    )"
    MANIFEST_HASH="$manifest_hash" TRANSCRIPT_HASH="$transcript_hash" python3 - "$path" <<'PY'
import json, os, pathlib, sys
pathlib.Path(sys.argv[1]).write_text(json.dumps({
  "schema_version": 2,
  "manifests": [{
    "hash": os.environ["MANIFEST_HASH"],
    "path": "corrections.md",
    "source": "output-directory"
  }],
  "source_hashes": {"transcript": os.environ["TRANSCRIPT_HASH"], "normalized": None},
  "rules": [{"find": "open telemetry", "replace": "OpenTelemetry", "source_manifest": 0, "applications": [{"artifact": "transcript.txt", "replacements": 1}] }]
}, indent=2) + "\n")
PY
    return
  }

  output="$workspace/eval-basic-transcribe/with_skill/outputs"
  mkdir -p "$output"
  printf 'An observability welcome.\n' > "$output/transcript.txt"
  : > "$output/subtitles.srt"
  : > "$output/subtitles.vtt"

  output="$workspace/eval-context-correction/with_skill/outputs"
  mkdir -p "$output"
  printf 'John Doe presents OpenTelemetry.\n' > "$output/transcript.txt"
  write_canonical_transcript "$output/transcript.json"
  write_correction_receipt "$output/corrections.applied.json"

  output="$workspace/eval-recursive-context/with_skill/outputs"
  mkdir -p "$output"
  printf 'open telemetry -> OpenTelemetry\n' > "$output/corrections.md"
  mkdir -p "$output/course/clip-01.cue" \
    "$output/course/module-02/clip-02.cue"
  printf 'John Doe presents OpenTelemetry.\n' \
    > "$output/course/clip-01.cue/transcript.txt"
  write_canonical_transcript "$output/course/clip-01.cue/transcript.json"
  write_correction_receipt \
    "$output/course/clip-01.cue/corrections.applied.json"
  printf 'John Doe presents OpenTelemetry.\n' \
    > "$output/course/module-02/clip-02.cue/transcript.txt"
  write_canonical_transcript \
    "$output/course/module-02/clip-02.cue/transcript.json"
  write_correction_receipt \
    "$output/course/module-02/clip-02.cue/corrections.applied.json"

  output="$workspace/eval-existing-correction/with_skill/outputs"
  mkdir -p "$output/1. welcome.cue" "$output/2. what we cover.cue"
  printf 'open telemetry -> OpenTelemetry\n' > "$output/corrections.md"
  printf 'John Doe presents OpenTelemetry.\n' \
    > "$output/1. welcome.cue/transcript.txt"
  printf 'OpenTelemetry\n' > "$output/1. welcome.cue/subtitles.srt"
  write_canonical_transcript "$output/1. welcome.cue/transcript.json"
  write_correction_receipt \
    "$output/1. welcome.cue/corrections.applied.json"
  mkdir -p "$workspace/.baselines"
  cp "$output/1. welcome.cue/transcript.json" \
    "$workspace/.baselines/with_skill-transcript.json"
  printf 'new transcript\n' > "$output/2. what we cover.cue/transcript.txt"

  output="$workspace/eval-explicit-multi/with_skill/outputs"
  mkdir -p "$output/clip-01.cue" "$output/clip-02.cue"
  printf 'first transcript\n' > "$output/clip-01.cue/transcript.txt"
  printf 'second transcript\n' > "$output/clip-02.cue/transcript.txt"

  output="$workspace/eval-lexicon-flywheel/with_skill/outputs"
  mkdir -p "$output/lesson.cue"
  write_canonical_transcript "$output/lesson.cue/transcript.json"
  cp "$output/lesson.cue/transcript.json" \
    "$workspace/.baselines/with_skill-flywheel-transcript.json"
  printf 'open telemetry -> OpenTelemetry\n' > "$output/corrections.md"
  write_correction_receipt "$output/lesson.cue/corrections.applied.json"
  printf '{"schema_version": 2, "output": "lesson.cue", "confidence_below": 0.75, "diagnostics": [{"id": "CUE-REVIEW-LOW-CONFIDENCE", "word": "open", "word_index": 0, "confidence": 0.4, "start_ms": 0, "end_ms": 300}]}\n' > "$output/review.json"
  RECEIPT_HASH="$(python3 - "$output/lesson.cue/corrections.applied.json" "$GRADER" <<'PY'
import importlib.util, pathlib, sys
spec = importlib.util.spec_from_file_location("grader", sys.argv[2])
grader = importlib.util.module_from_spec(spec); spec.loader.exec_module(grader)
print(grader.blake3_hash(pathlib.Path(sys.argv[1]).read_bytes()))
PY
  )"
  LEXICON_HASH="$(python3 - "$output/corrections.md" "$GRADER" <<'PY'
import importlib.util, pathlib, sys
spec = importlib.util.spec_from_file_location("grader", sys.argv[2])
grader = importlib.util.module_from_spec(spec); spec.loader.exec_module(grader)
print(grader.blake3_hash(pathlib.Path(sys.argv[1]).read_bytes()))
PY
  )"
  RECEIPT_HASH="$RECEIPT_HASH" LEXICON_HASH="$LEXICON_HASH" python3 - "$output/promotion.json" "$output/corrections.md" <<'PY'
import json, os, pathlib, sys
pathlib.Path(sys.argv[1]).write_text(json.dumps({
  "schema_version": 1,
  "status": "promoted",
  "source_receipt_hash": os.environ["RECEIPT_HASH"],
  "target_lexicon": "corrections.md",
  "target_lexicon_hash": os.environ["LEXICON_HASH"],
  "find": "open telemetry",
  "replace": "OpenTelemetry"
}, indent=2) + "\n")
PY

  output="$workspace/eval-recover-batch/with_skill/outputs"
  mkdir -p "$output"
  printf 'export CUE_STATE_DIR="$PWD/.cue-state"\n1. cue batches show batch-eval-recovery\n2. cue resume batch-eval-recovery\n3. cue batches show batch-eval-recovery\n' \
    > "$output/recovery-steps.txt"
  printf 'Batch remains incomplete: missing lesson-03.mp4\n' \
    > "$output/recovery-report.txt"
}

self_test() {
  local temporary success missing corrected_canonical missing_recursive_canonical
  local manual_sentinel_existing malformed_receipt missing_receipt nonempty
  local legacy_receipt stale_normalized tampered_rule malformed_review
  local fabricated_attestation unapplied_attestation broken_recovery_sequence
  local false_recovery_completion
  local before after canonical_before canonical_after
  local traversal_rubric other_path_rubric symlink_rubric outside
  local non_object_rubric non_object_eval_rubric grade_output grade_status
  temporary="$(mktemp -d "${TMPDIR:-/tmp}/cue-skill-eval-test.XXXXXX")"
  trap "rm -rf '$temporary'" EXIT
  success="$temporary/success"
  missing="$temporary/missing"
  nonempty="$temporary/nonempty"

  canonical_before="$(find "$ROOT/skills/transcribe/evals/files" -type f -exec cksum {} \; 2>/dev/null | sort | cksum)"
  python3 "$GRADER" --rubric "$RUBRIC" --validate-rubric >/dev/null

  mkdir -p "$nonempty"
  printf 'keep\n' > "$nonempty/sentinel"
  if "$0" --seed "$nonempty" >/dev/null 2>&1; then
    echo "FAIL  seed accepted a non-empty workspace" >&2
    return 1
  fi
  grep -q keep "$nonempty/sentinel"

  write_synthetic_success "$success"
  before="$(find "$success" -type f -exec cksum {} \; | sort | cksum)"
  "$0" --grade "$success" >/dev/null
  after="$(find "$success" -type f -exec cksum {} \; | sort | cksum)"
  [ "$before" = "$after" ] || {
    echo "FAIL  grade mutated its workspace" >&2
    return 1
  }

  cp -R "$success" "$missing"
  rm "$missing/eval-basic-transcribe/with_skill/outputs/transcript.txt"
  if "$0" --grade "$missing" >/dev/null 2>&1; then
    echo "FAIL  grade accepted missing output" >&2
    return 1
  fi

  corrected_canonical="$temporary/corrected-canonical"
  cp -R "$success" "$corrected_canonical"
  printf '{"text":"John Doe presents OpenTelemetry."}\n' \
    > "$corrected_canonical/eval-context-correction/with_skill/outputs/transcript.json"
  if "$0" --grade "$corrected_canonical" >/dev/null 2>&1; then
    echo "FAIL  grade accepted a corrected canonical transcript" >&2
    return 1
  fi

  missing_recursive_canonical="$temporary/missing-recursive-canonical"
  cp -R "$success" "$missing_recursive_canonical"
  rm "$missing_recursive_canonical/eval-recursive-context/with_skill/outputs/course/clip-01.cue/transcript.json"
  if "$0" --grade "$missing_recursive_canonical" >/dev/null 2>&1; then
    echo "FAIL  grade accepted a missing top-level recursive canonical transcript" >&2
    return 1
  fi

  manual_sentinel_existing="$temporary/manual-sentinel-existing"
  cp -R "$success" "$manual_sentinel_existing"
  printf 'PRE-EXISTING OUTPUT SENTINEL\n' \
    >> "$manual_sentinel_existing/eval-existing-correction/with_skill/outputs/1. welcome.cue/transcript.txt"
  if "$0" --grade "$manual_sentinel_existing" >/dev/null 2>&1; then
    echo "FAIL  grade accepted a manual derived-file sentinel" >&2
    return 1
  fi

  missing_receipt="$temporary/missing-receipt"
  cp -R "$success" "$missing_receipt"
  rm "$missing_receipt/eval-context-correction/with_skill/outputs/corrections.applied.json"
  if "$0" --grade "$missing_receipt" >/dev/null 2>&1; then
    echo "FAIL  grade accepted a corrected transcript without a receipt" >&2
    return 1
  fi

  malformed_receipt="$temporary/malformed-receipt"
  cp -R "$success" "$malformed_receipt"
  python3 -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); d["schema_version"]=True; del d["source_hashes"]["normalized"]; open(p,"w").write(json.dumps(d))' \
    "$malformed_receipt/eval-context-correction/with_skill/outputs/corrections.applied.json"
  if "$0" --grade "$malformed_receipt" >/dev/null 2>&1; then
    echo "FAIL  grade accepted invalid receipt field types and omissions" >&2
    return 1
  fi

  legacy_receipt="$temporary/legacy-receipt"
  cp -R "$success" "$legacy_receipt"
  python3 -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); m=d.pop("manifests")[0]; d["schema_version"]=1; d["manifest_hash"]=m["hash"]; d["manifest_path"]=m["path"]; d["manifest_source"]=m["source"]; [r.pop("source_manifest") for r in d["rules"]]; open(p,"w").write(json.dumps(d))' \
    "$legacy_receipt/eval-context-correction/with_skill/outputs/corrections.applied.json"
  if ! "$0" --grade "$legacy_receipt" >/dev/null 2>&1; then
    echo "FAIL  grade rejected a valid legacy schema-v1 receipt" >&2
    return 1
  fi

  stale_normalized="$temporary/stale-normalized"
  cp -R "$success" "$stale_normalized"
  printf '{"schema_version":1,"chunks":[]}\n' \
    > "$stale_normalized/eval-context-correction/with_skill/outputs/normalized.json"
  if "$0" --grade "$stale_normalized" >/dev/null 2>&1; then
    echo "FAIL  grade accepted normalized state absent from the receipt" >&2
    return 1
  fi

  tampered_rule="$temporary/tampered-rule"
  cp -R "$success" "$tampered_rule"
  python3 -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); d["rules"][0]["replace"]="Fabricated"; open(p,"w").write(json.dumps(d))' \
    "$tampered_rule/eval-context-correction/with_skill/outputs/corrections.applied.json"
  if "$0" --grade "$tampered_rule" >/dev/null 2>&1; then
    echo "FAIL  grade accepted a receipt rule absent from its manifest" >&2
    return 1
  fi

  malformed_review="$temporary/malformed-review"
  cp -R "$success" "$malformed_review"
  printf '{"schema_version": 1, "diagnostics": []}\n' \
    > "$malformed_review/eval-lexicon-flywheel/with_skill/outputs/review.json"
  if "$0" --grade "$malformed_review" >/dev/null 2>&1; then
    echo "FAIL  grade accepted an incomplete review report" >&2
    return 1
  fi

  fabricated_attestation="$temporary/fabricated-attestation"
  cp -R "$success" "$fabricated_attestation"
  python3 - "$fabricated_attestation/eval-lexicon-flywheel/with_skill/outputs" "$GRADER" <<'PY'
import importlib.util, json, pathlib, sys
output = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("grader", sys.argv[2])
grader = importlib.util.module_from_spec(spec); spec.loader.exec_module(grader)
lexicon = output / "corrections.md"
lexicon.write_text(lexicon.read_text() + "fabricated -> Fabricated\n")
attestation_path = output / "promotion.json"
attestation = json.loads(attestation_path.read_text())
attestation.update({
    "target_lexicon_hash": grader.blake3_hash(lexicon.read_bytes()),
    "find": "fabricated",
    "replace": "Fabricated",
})
attestation_path.write_text(json.dumps(attestation))
PY
  if "$0" --grade "$fabricated_attestation" >/dev/null 2>&1; then
    echo "FAIL  grade accepted an attestation for a rule absent from its receipt" >&2
    return 1
  fi

  unapplied_attestation="$temporary/unapplied-attestation"
  cp -R "$success" "$unapplied_attestation"
  python3 - "$unapplied_attestation/eval-lexicon-flywheel/with_skill/outputs" "$GRADER" <<'PY'
import importlib.util, json, pathlib, sys
output = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("grader", sys.argv[2])
grader = importlib.util.module_from_spec(spec); spec.loader.exec_module(grader)
receipt_path = output / "lesson.cue" / "corrections.applied.json"
receipt = json.loads(receipt_path.read_text())
receipt["rules"][0]["applications"][0]["replacements"] = 0
receipt_path.write_text(json.dumps(receipt))
attestation_path = output / "promotion.json"
attestation = json.loads(attestation_path.read_text())
attestation["source_receipt_hash"] = grader.blake3_hash(receipt_path.read_bytes())
attestation_path.write_text(json.dumps(attestation))
PY
  if "$0" --grade "$unapplied_attestation" >/dev/null 2>&1; then
    echo "FAIL  grade accepted an attestation for a rule with no replacements" >&2
    return 1
  fi

  broken_recovery_sequence="$temporary/broken-recovery-sequence"
  cp -R "$success" "$broken_recovery_sequence"
  printf '1. cue resume\n2. cue batches show\n' \
    > "$broken_recovery_sequence/eval-recover-batch/with_skill/outputs/recovery-steps.txt"
  if "$0" --grade "$broken_recovery_sequence" >/dev/null 2>&1; then
    echo "FAIL  grade accepted recovery without inspect-resume-inspect sequencing" >&2
    return 1
  fi

  false_recovery_completion="$temporary/false-recovery-completion"
  cp -R "$success" "$false_recovery_completion"
  printf 'Batch complete; missing lesson-03.mp4\n' \
    > "$false_recovery_completion/eval-recover-batch/with_skill/outputs/recovery-report.txt"
  if "$0" --grade "$false_recovery_completion" >/dev/null 2>&1; then
    echo "FAIL  grade accepted a completion claim with missing work" >&2
    return 1
  fi

  traversal_rubric="$temporary/traversal-rubric.json"
  cat > "$traversal_rubric" <<'EOF'
{"evals":[{"id":1,"assertions":[{"name":"escape","type":"file_exists","path":"../outside"}]}]}
EOF
  if python3 "$GRADER" --rubric "$traversal_rubric" --workspace "$success" >/dev/null 2>&1; then
    echo "FAIL  grader accepted a path outside the workspace" >&2
    return 1
  fi

  other_path_rubric="$temporary/other-path-rubric.json"
  cat > "$other_path_rubric" <<'EOF'
{"evals":[{"id":1,"assertions":[{"name":"other escape","type":"files_equal","path":"eval-basic-transcribe/with_skill/outputs/transcript.txt","other_path":"../outside"}]}]}
EOF
  if python3 "$GRADER" --rubric "$other_path_rubric" --workspace "$success" >/dev/null 2>&1; then
    echo "FAIL  grader accepted an other_path outside the workspace" >&2
    return 1
  fi

  outside="$temporary/outside"
  mkdir -p "$outside"
  printf 'secret\n' > "$outside/secret.txt"
  ln -s "$outside" "$success/outside-link"
  symlink_rubric="$temporary/symlink-rubric.json"
  cat > "$symlink_rubric" <<'EOF'
{"evals":[{"id":1,"assertions":[{"name":"symlink escape","type":"file_exists","path":"outside-link/secret.txt"}]}]}
EOF
  if python3 "$GRADER" --rubric "$symlink_rubric" --workspace "$success" >/dev/null 2>&1; then
    echo "FAIL  grader followed a symlink outside the workspace" >&2
    return 1
  fi

  non_object_rubric="$temporary/non-object-rubric.json"
  printf '[]\n' > "$non_object_rubric"
  set +e
  grade_output="$(python3 "$GRADER" --rubric "$non_object_rubric" --workspace "$success" 2>&1)"
  grade_status=$?
  set -e
  if [ "$grade_status" -ne 2 ] || ! printf '%s\n' "$grade_output" | grep -q '^grade error: rubric must be an object$'; then
    echo "FAIL  grader did not report a non-object rubric as a grade error" >&2
    return 1
  fi

  non_object_eval_rubric="$temporary/non-object-eval-rubric.json"
  printf '{"evals":[[]]}\n' > "$non_object_eval_rubric"
  set +e
  grade_output="$(python3 "$GRADER" --rubric "$non_object_eval_rubric" --workspace "$success" 2>&1)"
  grade_status=$?
  set -e
  if [ "$grade_status" -ne 2 ] || ! printf '%s\n' "$grade_output" | grep -q '^grade error: every eval must be an object$'; then
    echo "FAIL  grader did not report a non-object eval as a grade error" >&2
    return 1
  fi

  canonical_after="$(find "$ROOT/skills/transcribe/evals/files" -type f -exec cksum {} \; 2>/dev/null | sort | cksum)"
  [ "$canonical_before" = "$canonical_after" ] || {
    echo "FAIL  self-test wrote canonical eval fixtures" >&2
    return 1
  }

  echo "skill eval harness self-test ok"
  rm -rf "$temporary"
  trap - EXIT
}

case "${1:-}" in
  --seed)
    [ "$#" -le 2 ] || usage
    workspace="${2:-}"
    [ -n "$workspace" ] || workspace="$(new_workspace)"
    seed_workspace "$workspace"
    ;;
  --grade)
    [ "$#" -eq 2 ] || usage
    [ -d "$2" ] || { echo "workspace does not exist: $2" >&2; exit 2; }
    python3 "$GRADER" --rubric "$RUBRIC" --workspace "$2"
    ;;
  --self-test)
    [ "$#" -eq 1 ] || usage
    self_test
    ;;
  *) usage ;;
esac
