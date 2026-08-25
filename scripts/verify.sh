#!/bin/bash
# cue full verification suite.
#
# Runs every build gate and an end-to-end behavioral battery against a mock
# gateway (scripts/testdata/mock_gateway.py). Prints PASS/FAIL per check;
# exits non-zero on any failure.
#
# Usage: scripts/verify.sh

set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CUE="$ROOT/target/debug/cue"
MOCK="$ROOT/scripts/testdata/mock_gateway.py"
CFG_DIR="/tmp/cue-verify-cfg"
OUT="/tmp/cue-verify-out"
FAILURES=0

check() {
  local name="$1"; shift
  if "$@" > /tmp/cue-verify-last.log 2>&1; then
    echo "PASS  $name"
  else
    echo "FAIL  $name"
    echo "------ last output:"
    tail -20 /tmp/cue-verify-last.log
    FAILURES=$((FAILURES+1))
  fi
}

check_fail() {
  # A command that must exit non-zero.
  local name="$1"; shift
  if "$@" > /tmp/cue-verify-last.log 2>&1; then
    echo "FAIL  $name (expected non-zero exit)"
    FAILURES=$((FAILURES+1))
  else
    echo "PASS  $name"
  fi
}

echo "== build gates =="
check "fmt"        bash -c "cd $ROOT && cargo fmt --check"
check "build"      bash -c "cd $ROOT && cargo build --workspace --all-targets -q"
check "test"       bash -c "cd $ROOT && cargo test --workspace -q"
check "clippy"     bash -c "cd $ROOT && cargo clippy --workspace --all-targets -q"

echo
echo "== cli basics =="
check "--help"              $CUE --help
check "--version"           $CUE --version
check "spec emission"       bash -c "$CUE __usage_spec__ | grep -q doctor"
check "unknown flag"        bash -c "! $CUE --bogus 2>/dev/null"

echo
echo "== error paths =="
rm -rf /tmp/cue-verify-missing
check_fail "missing file"   $CUE /tmp/cue-verify-missing.mp4 --output /tmp/x
printf 'garbage, not media' > /tmp/cue-verify-garbage.mp4
check_fail "corrupt media"  $CUE /tmp/cue-verify-garbage.mp4 --output /tmp/x
ffmpeg -y -v error -f lavfi -i "color=black:size=64x64:duration=1" /tmp/cue-verify-silent.mp4
check_fail "no audio stream" $CUE /tmp/cue-verify-silent.mp4 --output /tmp/x
check_fail "bad model name"  $CUE models install whisper

echo
echo "== end-to-end pipeline (mock gateway) =="
python3 "$MOCK" &
GW=$!
VERIFY_DIR=""
trap 'kill $GW 2>/dev/null || true; rm -rf "$VERIFY_DIR" 2>/dev/null || true' EXIT
sleep 1

mkdir -p "$CFG_DIR"
cat > "$CFG_DIR/cue.toml" <<EOF
[transcription]
model = "tiny"

[normalization]
ollama_url = "http://127.0.0.1:8765"

[llm]
base_url = "http://127.0.0.1:8765/v1"
model = "test-model"
api_key_env = "CUE_TEST_KEY"
EOF

say -o /tmp/cue-verify-speech.aiff "Hello from the cue verification suite." \
  || { echo "FAIL  speech fixture (macOS 'say' required)"; exit 1; }
ffmpeg -y -v error -i /tmp/cue-verify-speech.aiff /tmp/cue-verify-speech.mp3
rm -rf "$OUT"

check "pipeline run" env CUE_CONFIG_DIR=$CFG_DIR $CUE /tmp/cue-verify-speech.mp3 --output $OUT

for f in transcript.json transcript.txt transcript.clean.txt normalized.json \
         subtitles.srt subtitles.vtt analysis.json summary.md description.md; do
  check "output exists: $f" test -s "$OUT/$f"
done

check "transcript has words"     bash -c "python3 -c \"import json; d=json.load(open('$OUT/transcript.json')); assert len(d['words'])>0\""
check "clean text is non-empty"  bash -c "grep -q . '$OUT/transcript.clean.txt'"
check "srt well-formed"          bash -c "grep -q -- '-->' '$OUT/subtitles.srt'"
check "vtt has header"           bash -c "head -1 '$OUT/subtitles.vtt' | grep -q WEBVTT"
check "analysis schema version"  bash -c "grep -q '\"schema_version\": 1' '$OUT/analysis.json'"
check "summary mentions title"   bash -c "grep -q 'Cue Pipeline Test' '$OUT/summary.md'"
check "description has chapters" bash -c "grep -q 'Chapters' '$OUT/description.md'"

echo
echo "== cache behavior =="
check "rerun fully cached" bash -c "env CUE_CONFIG_DIR=$CFG_DIR $CUE /tmp/cue-verify-speech.mp3 --output $OUT 2>&1 | grep -c cached | grep -q 4"

check "doctor optional ok" bash -c "env CUE_CONFIG_DIR=$CFG_DIR $CUE doctor | grep -q 'S1.*ok.*ready'"
check "models list"        bash -c "env CUE_CONFIG_DIR=$CFG_DIR $CUE models list | grep -q cue-s1-mini"
check "models check ok"    bash -c "env CUE_CONFIG_DIR=$CFG_DIR $CUE models check"

echo
echo "== transcribe subcommand =="
rm -rf /tmp/cue-verify-trans
check "transcribe runs" env CUE_CONFIG_DIR=$CFG_DIR $CUE transcribe /tmp/cue-verify-speech.mp3 --output /tmp/cue-verify-trans
check "transcript exists"     test -s /tmp/cue-verify-trans/transcript.txt
check_fail "no subtitles"     test -s /tmp/cue-verify-trans/subtitles.srt
check_fail "no analysis"      test -s /tmp/cue-verify-trans/analysis.json

echo
echo "== skill =="
check "skill help"         $CUE skill --help
check "skill argv"         bash -c "grep -q 'npx skills add' crates/cue-cli/src/commands/skill.rs"
check "SKILL.md frontmatter" bash -c "grep -q '^name: transcribe' skills/transcribe/SKILL.md && grep -q '^description:' skills/transcribe/SKILL.md"
check "evals.json valid"   bash -c "python3 -c \"import json; d=json.load(open('skills/transcribe/evals/evals.json')); assert len(d['evals'])>=2; assert all(c['assertions'] for c in d['evals'])\""
check_fail "no real identifiers" bash -c "grep -riE 'eastham|dometrain' skills/transcribe/ || exit 1; exit 0"

echo
echo "== correct command =="
VERIFY_DIR=$(mktemp -d /tmp/cue-verify-correct.XXXXXX)
mkdir -p "$VERIFY_DIR/talk.cue"
printf 'I am John Dough. See open telemetry.\n' > "$VERIFY_DIR/talk.cue/transcript.txt"
printf 'open telemetry is key.\n' > "$VERIFY_DIR/talk.cue/subtitles.srt"
printf '{"schema_version":1}\n' > "$VERIFY_DIR/talk.cue/transcript.json"
printf 'John Dough -> John Doe\nopen telemetry -> OpenTelemetry\n' > "$VERIFY_DIR/corrections.md"
BEFORE=$(shasum -a 256 "$VERIFY_DIR/talk.cue/transcript.txt" "$VERIFY_DIR/talk.cue/subtitles.srt" "$VERIFY_DIR/talk.cue/transcript.json" | shasum | cut -d' ' -f1)
check "correct dry-run writes nothing" bash -c "$CUE correct $VERIFY_DIR/talk.cue --dry-run 2>&1 | grep -q 'Dry run'"
AFTER=$(shasum -a 256 "$VERIFY_DIR/talk.cue/transcript.txt" "$VERIFY_DIR/talk.cue/subtitles.srt" "$VERIFY_DIR/talk.cue/transcript.json" | shasum | cut -d' ' -f1)
check "correct dry-run leaves all artifacts unchanged" test "$BEFORE" = "$AFTER"
check "correct applies"               bash -c "$CUE correct $VERIFY_DIR/talk.cue 2>&1 | grep -q 'replacement(s)'"
check "correct fixed transcript"      bash -c "grep -q 'OpenTelemetry' $VERIFY_DIR/talk.cue/transcript.txt && ! grep -q 'John Dough' $VERIFY_DIR/talk.cue/transcript.txt"
check "correct fixed subtitles"       bash -c "grep -q 'OpenTelemetry' $VERIFY_DIR/talk.cue/subtitles.srt"
check "correct kept json"             bash -c "grep -q 'schema_version' $VERIFY_DIR/talk.cue/transcript.json"

kill $GW 2>/dev/null || true

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "ALL CHECKS PASSED"
else
  echo "$FAILURES CHECK(S) FAILED"
fi
exit $FAILURES
