#!/bin/bash
# Run the transcribe skill evals.
#
# Layout per the agentskills evaluation guide:
#   skills/transcribe-workspace/iteration-N/<case>/{with,without}_skill/
#
# The skill is installed via `npx skills add` (the real user path), so the
# eval is harness-agnostic: whichever agent executes each prompt picks the
# skill up from its normal skills directory. The script only does what is
# deterministic: build fixtures, lay out workspaces, run cue directly when
# needed, and grade mechanical assertions.
#
# Usage:
#   scripts/skill-eval-harness.sh           # generate fixtures + cue runs + prompts
#   scripts/skill-eval-harness.sh --grade   # grade an EXISTING workspace (no re-seed)
#
# The with_skill/without_skill *agent* runs are prompts you execute in your
# agent (any harness). See iteration output below.

set -eu
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKILL_DIR="$ROOT/skills/transcribe"
FILES_DIR="$SKILL_DIR/evals/files"
ITER="$ROOT/skills/transcribe-workspace/iteration-1"

GRADE=0
[ "${1:-}" = "--grade" ] && GRADE=1

if [ "$GRADE" = "0" ]; then
  # Setup only runs when seeding a fresh workspace; grading skips it so a
  # `--grade` pass never wipes agent-applied corrections.
  mkdir -p "$FILES_DIR"

# ---- 1. Generate deterministic speech fixtures (macOS `say` + ffmpeg) ----
# Spoken content uses the placeholder speaker "John Doe" and the public
# project name "OpenTelemetry", which ASR reliably lowercases/splits
# ("open telemetry"); the context file gives agents the correct spelling.
# All identities are clearly fictional; nothing personal or paid is echoed.
gen_clip() {
  local text="$1" out="$2"
  say -o "$out.aiff" "$text"
  ffmpeg -y -v error -i "$out.aiff" -ar 16000 -ac 1 "$out.mp3"
  rm -f "$out.aiff"
}

gen_clip \
  "Hi there, I'm John Doe and this is the observability workshop. Welcome to Acme Dev Conf." \
  "$FILES_DIR/clip-01"
gen_clip \
  "Welcome back, I'm John Doe again. Today we cover spans, metrics, and traces with open telemetry." \
  "$FILES_DIR/clip-02"

cat > "$FILES_DIR/context.md" <<'EOF'
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

echo "fixtures ready: $FILES_DIR"

# ---- 2. Direct cue transcription runs (deterministic inputs) ------------
# Each case's input media is transcribed with cue up front. The agent eval
# then reads/corrects these outputs, so the with/without contrast isolates
# the skill's behavior rather than whisper's nondeterminism.
mkdir -p "$ITER"
for case in eval-basic-transcribe eval-context-correction eval-batch-context eval-existing-correction; do
  for variant in with_skill without_skill; do
    mkdir -p "$ITER/$case/$variant/outputs"
  done
done

run_cue() {
  local clip="$1" outdir="$2"
  # Full cue (not `cue transcribe`): matches what the skill teaches as the
  # default, producing subtitles alongside the transcript.
  "$ROOT/target/debug/cue" "$FILES_DIR/$clip.mp3" --output "$outdir" > /dev/null 2>&1
}

run_cue clip-01 "$ITER/eval-basic-transcribe/with_skill/outputs"
run_cue clip-01 "$ITER/eval-basic-transcribe/without_skill/outputs"
run_cue clip-02 "$ITER/eval-context-correction/with_skill/outputs"
run_cue clip-02 "$ITER/eval-context-correction/without_skill/outputs"
run_cue clip-01 "$ITER/eval-batch-context/with_skill/outputs/clip-01"
run_cue clip-02 "$ITER/eval-batch-context/with_skill/outputs/clip-02"
run_cue clip-01 "$ITER/eval-batch-context/without_skill/outputs/clip-01"
run_cue clip-02 "$ITER/eval-batch-context/without_skill/outputs/clip-02"

# ---- 3. Seed the "existing outputs" case (eval 4) -----------------------
# Simulates the situation the skill must handle: an already-transcribed
# folder whose transcript.txt (and subtitles) contain a genuine mishearing
# from clip-02 ("open telemetry"), plus a NEW sibling clip to transcribe.
# The pre-existing transcript.json must stay untouched.
seed_existing() {
  local variant="$1"
  local dir="$ITER/eval-existing-correction/$variant/outputs"
  mkdir -p "$dir/1. welcome.cue" "$dir"

  # Pre-existing output transcribed from clip-02: whisper genuinely writes
  # "open telemetry" here, so transcript and subtitles both carry the garble.
  run_cue clip-02 "$dir/1. welcome.cue"

  # The new sibling clip to transcribe sits in the same folder.
  cp "$FILES_DIR/clip-01.mp3" "$dir/2. what we cover.mp3"
}
seed_existing with_skill
seed_existing without_skill

# Copy the context file into the correction cases (the skill tells agents to
# look for it next to the media).
cp "$FILES_DIR/context.md" "$ITER/eval-context-correction/with_skill/outputs/"
cp "$FILES_DIR/context.md" "$ITER/eval-context-correction/without_skill/outputs/"
cp "$FILES_DIR/context.md" "$ITER/eval-batch-context/with_skill/outputs/"
cp "$FILES_DIR/context.md" "$ITER/eval-batch-context/without_skill/outputs/"
cp "$FILES_DIR/context.md" "$ITER/eval-existing-correction/with_skill/outputs/"
cp "$FILES_DIR/context.md" "$ITER/eval-existing-correction/without_skill/outputs/"

echo "cue runs done"

# ---- 3. Prompts for the agent harness --------------------------------
# Run each prompt in your agent (any harness). For with_skill runs, the
# skill must be installed:  npx skills add nkootstra/cue
# For without_skill runs, the same prompt with the skill NOT installed.
cat <<EOF

=== Agent prompts (execute each in your agent) ===

[basic-transcribe] with_skill
  Prompt: Can you transcribe this conference welcome clip for me?
  Input:  $ITER/eval-basic-transcribe/with_skill/outputs/clip-01.mp3
  Save outputs to: $ITER/eval-basic-transcribe/with_skill/outputs

[basic-transcribe] without_skill
  Prompt: Can you transcribe this conference welcome clip for me?
  Input:  $ITER/eval-basic-transcribe/without_skill/outputs/clip-01.mp3
  Save outputs to: $ITER/eval-basic-transcribe/without_skill/outputs

[context-correction] with_skill
  Prompt: Transcribe this conference welcome clip. A context file
  describing the talk is next to the clip — use it to fix any misheard
  names in the transcript.
  Input:  $ITER/eval-context-correction/with_skill/outputs/
  Save outputs to: $ITER/eval-context-correction/with_skill/outputs

[context-correction] without_skill
  Prompt: Transcribe this conference welcome clip.
  Input:  $ITER/eval-context-correction/without_skill/outputs/
  Save outputs to: $ITER/eval-context-correction/without_skill/outputs

[batch-context] with_skill
  Prompt: There are two welcome clips and one shared context file in the
  current directory. Transcribe both and apply the shared speaker context
  to both transcripts.
  Input:  $ITER/eval-batch-context/with_skill/outputs/
  Save outputs to: $ITER/eval-batch-context/with_skill/outputs

[batch-context] without_skill
  Prompt: Transcribe the two welcome clips in the current directory.
  Input:  $ITER/eval-batch-context/without_skill/outputs/
  Save outputs to: $ITER/eval-batch-context/without_skill/outputs

[existing-correction] with_skill
  Prompt: There is a media file ("2. what we cover.mp3") and an
  already-transcribed folder ("1. welcome.cue") in the current directory,
  plus a shared context file. Transcribe the new clip, then write a
  corrections.md manifest for the misheard term and apply it with
  cue correct to the existing folder.
  Input:  $ITER/eval-existing-correction/with_skill/outputs/
  Save outputs to: $ITER/eval-existing-correction/with_skill/outputs

[existing-correction] without_skill
  Prompt: There is a media file and an already-transcribed folder in the
  current directory. Transcribe the new clip.
  Input:  $ITER/eval-existing-correction/without_skill/outputs/
  Save outputs to: $ITER/eval-existing-correction/without_skill/outputs

=== Then run: scripts/skill-eval-harness.sh --grade ===
EOF

fi # end of setup (skipped when grading)

if [ "$GRADE" = "1" ]; then
  echo "=== grading (per evals.json assertions) ==="
  PASS=0; FAIL=0
  grade() { # ok name
    if [ "$1" = "1" ]; then PASS=$((PASS+1)); echo "PASS  $2"; else FAIL=$((FAIL+1)); echo "FAIL  $2"; fi
  }

  # case 1: basic-transcribe (clip-01)
  T="$ITER/eval-basic-transcribe/with_skill/outputs/transcript.txt"
  [ -s "$T" ] && grade 1 "case1 transcript non-empty" || grade 0 "case1 transcript non-empty"
  grep -qi observability "$T" && grade 1 "case1 contains observability" || grade 0 "case1 contains observability"
  [ -f "$ITER/eval-basic-transcribe/with_skill/outputs/subtitles.srt" ] \
    && [ -f "$ITER/eval-basic-transcribe/with_skill/outputs/subtitles.vtt" ] \
    && grade 1 "case1 subtitles exist" || grade 0 "case1 subtitles exist"

  # case 2: context-correction (clip-02)
  T="$ITER/eval-context-correction/with_skill/outputs/transcript.txt"
  grep -qi "john doe" "$T" && grade 1 "case2 uses Doe" || grade 0 "case2 uses Doe"
  grep -qi opentelemetry "$T" && grade 1 "case2 uses OpenTelemetry" || grade 0 "case2 uses OpenTelemetry"
  grep -qi "open telemetry" "$T" && grade 0 "case2 no split/lowercase garble" || grade 1 "case2 no split/lowercase garble"
  [ -f "$ITER/eval-context-correction/with_skill/outputs/transcript.json" ] \
    && grade 1 "case2 transcript.json intact" || grade 0 "case2 transcript.json intact"

  # case 3: batch-context
  for clip in clip-01 clip-02; do
    T="$ITER/eval-batch-context/with_skill/outputs/$clip/transcript.txt"
    [ -s "$T" ] && grade 1 "case3 $clip non-empty" || grade 0 "case3 $clip non-empty"
    grep -qi "john doe" "$T" && grade 1 "case3 $clip uses Doe" || grade 0 "case3 $clip uses Doe"
  done
  grep -qi opentelemetry "$ITER/eval-batch-context/with_skill/outputs/clip-02/transcript.txt" \
    && grade 1 "case3 clip-02 OpenTelemetry" || grade 0 "case3 clip-02 OpenTelemetry"

  # case 4: existing-outputs correction (manifest + cue correct)
  EX="$ITER/eval-existing-correction/with_skill/outputs"
  T="$EX/1. welcome.cue/transcript.txt"
  grep -qi "open telemetry -> opentelemetry" "$EX/corrections.md" \
    && grade 1 "case4 manifest maps the garbled term" || grade 0 "case4 manifest maps the garbled term"
  [ -s "$T" ] && grade 1 "case4 existing transcript non-empty" || grade 0 "case4 existing transcript non-empty"
  grep -qi opentelemetry "$T" && grade 1 "case4 existing transcript corrected" || grade 0 "case4 existing transcript corrected"
  grep -qi "open telemetry" "$T" && grade 0 "case4 no garble in transcript" || grade 1 "case4 no garble in transcript"
  grep -qi opentelemetry "$EX/1. welcome.cue/subtitles.srt" \
    && grade 1 "case4 subtitles corrected" || grade 0 "case4 subtitles corrected"
  [ -f "$EX/1. welcome.cue/transcript.json" ] \
    && grade 1 "case4 transcript.json intact" || grade 0 "case4 transcript.json intact"
  [ -s "$EX/2. what we cover.cue/transcript.txt" ] \
    && grade 1 "case4 new clip transcribed" || grade 0 "case4 new clip transcribed"

  echo "TOTAL: $PASS passed, $FAIL failed"
fi