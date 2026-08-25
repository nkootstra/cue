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
#   scripts/skill-eval-harness.sh           # generate fixtures + cue runs
#   scripts/skill-eval-harness.sh --grade   # also grade assertions
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
for case in eval-basic-transcribe eval-context-correction eval-batch-context; do
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

# Copy the context file into the correction cases (the skill tells agents to
# look for it next to the media).
cp "$FILES_DIR/context.md" "$ITER/eval-context-correction/with_skill/outputs/"
cp "$FILES_DIR/context.md" "$ITER/eval-context-correction/without_skill/outputs/"
cp "$FILES_DIR/context.md" "$ITER/eval-batch-context/with_skill/outputs/"
cp "$FILES_DIR/context.md" "$ITER/eval-batch-context/without_skill/outputs/"

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

=== Then run: scripts/skill-eval-harness.sh --grade ===
EOF

if [ "$GRADE" = "1" ]; then
  echo "=== grading ==="
  grade_case() {
    local dir="$1" label="$2"
    local txt="$dir/outputs/transcript.txt"
    local pass=0 fail=0
    local t
    if [ -f "$txt" ] && [ -s "$txt" ]; then
      if grep -qi observability "$txt"; then pass=$((pass+1)); else fail=$((fail+1)); fi
    else
      fail=$((fail+1))
    fi
    # Speaker spelling from context; OpenTelemetry is the garbled term.
    t=$(grep -ci "john doe" "$txt" 2>/dev/null || echo 0)
    o=$(grep -ci "opentelemetry" "$txt" 2>/dev/null || echo 0)
    [ "$t" -ge 1 ] && pass=$((pass+1)) || fail=$((fail+1))
    [ "$o" -ge 1 ] && pass=$((pass+1)) || fail=$((fail+1))
    echo "$label: pass=$pass fail=$fail"
  }
  grade_case "$ITER/eval-context-correction/with_skill" "correction-with"
  grade_case "$ITER/eval-context-correction/without_skill" "correction-without"
fi