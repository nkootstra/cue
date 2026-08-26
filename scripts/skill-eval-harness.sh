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
  for case in eval-basic-transcribe eval-context-correction eval-recursive-context eval-existing-correction eval-explicit-multi; do
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
    "$ROOT/target/debug/cue" "$fixtures/clip-02.mp3" \
      --output "$output/1. welcome.cue" >/dev/null 2>&1
    printf '\nPRE-EXISTING OUTPUT SENTINEL open telemetry\n' \
      >> "$output/1. welcome.cue/transcript.txt"
    printf '\nPRE-EXISTING OUTPUT SENTINEL open telemetry\n' \
      >> "$output/1. welcome.cue/subtitles.srt"
    mkdir -p "$workspace/.baselines"
    cp "$output/1. welcome.cue/transcript.json" \
      "$workspace/.baselines/$variant-transcript.json"
    cp "$fixtures/clip-02.mp3" "$output/1. welcome.mp3"
    cp "$fixtures/clip-01.mp3" "$output/2. what we cover.mp3"
    cp "$fixtures/context.md" "$output/"

    output="$workspace/eval-explicit-multi/$variant/outputs"
    cp "$fixtures/clip-01.mp3" "$fixtures/clip-02.mp3" "$output/"
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
  corrected output folder ("1. welcome.cue") in the current directory.
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

=== Then run: scripts/skill-eval-harness.sh --grade $workspace ===
EOF
}

write_synthetic_success() {
  local workspace="$1" output

  output="$workspace/eval-basic-transcribe/with_skill/outputs"
  mkdir -p "$output"
  printf 'An observability welcome.\n' > "$output/transcript.txt"
  : > "$output/subtitles.srt"
  : > "$output/subtitles.vtt"

  output="$workspace/eval-context-correction/with_skill/outputs"
  mkdir -p "$output"
  printf 'John Doe presents OpenTelemetry.\n' > "$output/transcript.txt"
  printf '{"text":"John Doe presents open telemetry."}\n' > "$output/transcript.json"

  output="$workspace/eval-recursive-context/with_skill/outputs"
  mkdir -p "$output"
  printf 'open telemetry -> OpenTelemetry\n' > "$output/corrections.md"
  mkdir -p "$output/course/clip-01.cue" \
    "$output/course/module-02/clip-02.cue"
  printf 'John Doe presents OpenTelemetry.\n' \
    > "$output/course/clip-01.cue/transcript.txt"
  printf '{"text":"John Doe presents open telemetry."}\n' \
    > "$output/course/clip-01.cue/transcript.json"
  printf 'John Doe presents OpenTelemetry.\n' \
    > "$output/course/module-02/clip-02.cue/transcript.txt"
  printf '{"text":"John Doe presents open telemetry."}\n' \
    > "$output/course/module-02/clip-02.cue/transcript.json"

  output="$workspace/eval-existing-correction/with_skill/outputs"
  mkdir -p "$output/1. welcome.cue" "$output/2. what we cover.cue"
  printf 'open telemetry -> OpenTelemetry\n' > "$output/corrections.md"
  printf 'OpenTelemetry\nPRE-EXISTING OUTPUT SENTINEL\n' \
    > "$output/1. welcome.cue/transcript.txt"
  printf 'OpenTelemetry\n' > "$output/1. welcome.cue/subtitles.srt"
  printf '{"schema_version":1}\n' > "$output/1. welcome.cue/transcript.json"
  mkdir -p "$workspace/.baselines"
  cp "$output/1. welcome.cue/transcript.json" \
    "$workspace/.baselines/with_skill-transcript.json"
  printf 'new transcript\n' > "$output/2. what we cover.cue/transcript.txt"

  output="$workspace/eval-explicit-multi/with_skill/outputs"
  mkdir -p "$output/clip-01.cue" "$output/clip-02.cue"
  printf 'first transcript\n' > "$output/clip-01.cue/transcript.txt"
  printf 'second transcript\n' > "$output/clip-02.cue/transcript.txt"
}

self_test() {
  local temporary success missing corrected_canonical missing_recursive_canonical
  local rerendered_existing nonempty
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

  rerendered_existing="$temporary/rerendered-existing"
  cp -R "$success" "$rerendered_existing"
  printf 'OpenTelemetry\n' \
    > "$rerendered_existing/eval-existing-correction/with_skill/outputs/1. welcome.cue/transcript.txt"
  if "$0" --grade "$rerendered_existing" >/dev/null 2>&1; then
    echo "FAIL  grade accepted a rerendered existing transcript" >&2
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
