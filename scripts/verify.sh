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
VERIFY_TMP="$(mktemp -d "${TMPDIR:-/tmp}/cue-verify.XXXXXX")"
CFG_DIR="$VERIFY_TMP/cfg"
OUT="$VERIFY_TMP/out"
WORKSPACE="$OUT/.cue/speech"
SPEECH_MP3="$VERIFY_TMP/speech.mp3"
FAILURES=0

check() {
  local name="$1"; shift
  if "$@" > "$VERIFY_TMP/last.log" 2>&1; then
    echo "PASS  $name"
  else
    echo "FAIL  $name"
    echo "------ last output:"
    tail -20 "$VERIFY_TMP/last.log"
    FAILURES=$((FAILURES+1))
  fi
}

check_fail() {
  # A command that must exit non-zero.
  local name="$1"; shift
  if "$@" > "$VERIFY_TMP/last.log" 2>&1; then
    echo "FAIL  $name (expected non-zero exit)"
    FAILURES=$((FAILURES+1))
  else
    echo "PASS  $name"
  fi
}

mtime_ns() {
  python3 -c 'import os, sys; print(os.stat(sys.argv[1]).st_mtime_ns)' "$1"
}

mtime_is_unchanged() {
  local path="$1"
  local expected="$2"
  test -f "$path" && test "$(mtime_ns "$path")" = "$expected"
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
trap 'kill "$GW" 2>/dev/null || true; rm -rf "$VERIFY_DIR" "$VERIFY_TMP" 2>/dev/null || true' EXIT
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
api_key_env = ""
EOF

SPEECH_AIFF="$VERIFY_TMP/speech.aiff"
say -o "$SPEECH_AIFF" "Hello from the cue verification suite." \
  || { echo "FAIL  speech fixture (macOS 'say' required)"; exit 1; }
ffmpeg -y -v error -i "$SPEECH_AIFF" "$SPEECH_MP3"
rm -rf "$OUT"

check "pipeline run" env "CUE_CONFIG_DIR=$CFG_DIR" "$CUE" "$SPEECH_MP3" \
  --output "$OUT" --format srt --format vtt

for f in cue.workspace.json transcript.json transcript.txt transcript.clean.txt normalized.json \
         subtitles.srt subtitles.vtt analysis.json summary.md description.md \
         cue.run.json; do
  check "workspace output exists: $f" test -s "$WORKSPACE/$f"
done

check "published srt exists"      test -s "$OUT/speech.srt"
check "published vtt exists"      test -s "$OUT/speech.vtt"
check "transcript has words"     bash -c "python3 -c \"import json; d=json.load(open('$WORKSPACE/transcript.json')); assert len(d['words'])>0\""
check "clean text is non-empty"  bash -c "grep -q . '$WORKSPACE/transcript.clean.txt'"
check "srt well-formed"          bash -c "grep -q -- '-->' '$OUT/speech.srt'"
check "vtt has header"           bash -c "head -1 '$OUT/speech.vtt' | grep -q WEBVTT"
check "analysis schema version"  bash -c "grep -q '\"schema_version\": 1' '$WORKSPACE/analysis.json'"
check "summary mentions title"   bash -c "grep -q 'Cue Pipeline Test' '$WORKSPACE/summary.md'"
check "description has chapters" bash -c "grep -q 'Chapters' '$WORKSPACE/description.md'"
check "run receipt contract" bash -c "python3 -c \"import json; d=json.load(open('$WORKSPACE/cue.run.json')); assert d['schema_version']==3; assert d['mode']=='full'; assert d['source']['digest']['algorithm']=='blake3'; assert len(d['source']['digest']['value'])==64; assert 'batch_attempt' not in d; assert d['remote_data_usage']['normalized_text_sent_to_remote_in_current_run'] is None; assert {a['path'] for a in d['artifacts']} == {'cue.workspace.json','transcript.json','transcript.txt','transcript.clean.txt','normalized.json','subtitles.srt','subtitles.vtt','analysis.json','summary.md','description.md'}; assert {a['path'] for a in d['published_outputs']} == {'../../speech.srt','../../speech.vtt'}; assert {s['stage'] for s in d['stages']} >= {'inspect','extract','transcribe','normalize','analyze','render'}\""
check "verify accepts intact output" "$CUE" verify "$WORKSPACE"
check "verify JSON remains schema v2" bash -c "'$CUE' verify '$WORKSPACE' --json | python3 -c \"import json,sys; d=json.load(sys.stdin); assert d['schema_version']==2; assert d['valid'] is True\""
printf 'tampered\n' >> "$WORKSPACE/transcript.txt"
check_fail "verify detects artifact drift" "$CUE" verify "$WORKSPACE"
check "rerun restores attested output" env "CUE_CONFIG_DIR=$CFG_DIR" "$CUE" \
  "$SPEECH_MP3" --output "$OUT" --format srt --format vtt
check "verify accepts restored output" "$CUE" verify "$WORKSPACE"

echo
echo "== cache behavior =="
check "rerun fully cached" bash -c "env CUE_CONFIG_DIR=$CFG_DIR $CUE '$SPEECH_MP3' --output '$OUT' --format srt --format vtt 2>&1 | grep -c cached | grep -q 4"

SOURCE_TERM=$(python3 -c "import json,re; d=json.load(open('$WORKSPACE/transcript.json')); m=re.search(r'[A-Za-z0-9]+', d['segments'][0]['text']); assert m; print(m.group(0))")
PIPELINE_CANONICAL_HASH=$(shasum -a 256 "$WORKSPACE/transcript.json" "$WORKSPACE/analysis.json" "$WORKSPACE/normalized.json" | shasum | cut -d' ' -f1)
printf '%s -> DurableFirst\n' "$SOURCE_TERM" > "$WORKSPACE/corrections.md"
check "manifest applies on cached rerun" env "CUE_CONFIG_DIR=$CFG_DIR" "$CUE" \
  "$SPEECH_MP3" --output "$OUT" --format srt --format vtt
check "corrected render uses first manifest" bash -c "grep -q 'DurableFirst' '$WORKSPACE/transcript.txt' && grep -q 'DurableFirst' '$OUT/speech.srt'"
check "correction receipt is valid" bash -c "python3 -c \"import json; d=json.load(open('$WORKSPACE/corrections.applied.json')); assert d['schema_version']==2; assert d['manifests'][0]['source']=='output-directory'; assert d['source_hashes']['transcript']; assert d['rules'][0]['replace']=='DurableFirst'; assert d['rules'][0]['source_manifest']==0\""
check "first corrected rerun keeps canonical data" bash -c "test '$PIPELINE_CANONICAL_HASH' = \"\$(shasum -a 256 '$WORKSPACE/transcript.json' '$WORKSPACE/analysis.json' '$WORKSPACE/normalized.json' | shasum | cut -d' ' -f1)\""

printf '%s -> DurableSecond\n' "$SOURCE_TERM" > "$WORKSPACE/corrections.md"
check "changed manifest reapplies from canonical" env "CUE_CONFIG_DIR=$CFG_DIR" "$CUE" \
  "$SPEECH_MP3" --output "$OUT" --format srt --format vtt
check "changed manifest replaces prior correction" bash -c "grep -q 'DurableSecond' '$WORKSPACE/transcript.txt' && ! grep -q 'DurableFirst' '$WORKSPACE/transcript.txt'"

rm "$WORKSPACE/corrections.md"
check "manifest removal rerenders canonical text" env "CUE_CONFIG_DIR=$CFG_DIR" "$CUE" \
  "$SPEECH_MP3" --output "$OUT" --format srt --format vtt
check "manifest removal restores raw-derived text" bash -c "grep -Fqi '$SOURCE_TERM' '$WORKSPACE/transcript.txt' && ! grep -q 'DurableFirst\|DurableSecond' '$WORKSPACE/transcript.txt'"
check_fail "manifest removal clears receipt" test -e "$WORKSPACE/corrections.applied.json"
check "correction lifecycle keeps canonical data" bash -c "test '$PIPELINE_CANONICAL_HASH' = \"\$(shasum -a 256 '$WORKSPACE/transcript.json' '$WORKSPACE/analysis.json' '$WORKSPACE/normalized.json' | shasum | cut -d' ' -f1)\""

check "doctor optional ok" bash -c "env 'CUE_CONFIG_DIR=$CFG_DIR' '$CUE' doctor | grep -q 'S1.*ok.*ready'"
check "models list"        bash -c "env 'CUE_CONFIG_DIR=$CFG_DIR' '$CUE' models list | grep -q cue-s1-mini"
check "models check ok"    env "CUE_CONFIG_DIR=$CFG_DIR" "$CUE" models check

echo
echo "== transcribe subcommand =="
TRANS_DIR="$VERIFY_TMP/trans"
TRANS_WORKSPACE="$TRANS_DIR/.cue/speech"
rm -rf "$TRANS_DIR"
check "transcribe runs" env "CUE_CONFIG_DIR=$CFG_DIR" "$CUE" transcribe "$SPEECH_MP3" --output "$TRANS_DIR"
check "transcript exists"     test -s "$TRANS_WORKSPACE/transcript.txt"
check "transcribe receipt exists" test -s "$TRANS_WORKSPACE/cue.run.json"
check "transcribe receipt mode" bash -c "python3 -c \"import json; d=json.load(open('$TRANS_WORKSPACE/cue.run.json')); assert d['schema_version']==3; assert d['mode']=='transcript-only'; assert 'batch_attempt' not in d; assert {a['path'] for a in d['artifacts']} == {'cue.workspace.json','transcript.json','transcript.txt'}; assert d['published_outputs'] == []\""
check "verify accepts transcript-only output" "$CUE" verify "$TRANS_WORKSPACE"
check_fail "no subtitles"     test -s "$TRANS_WORKSPACE/subtitles.srt"
check_fail "no analysis"      test -s "$TRANS_WORKSPACE/analysis.json"

echo
echo "== recoverable batch operations =="
RECOVERY_ROOT="$VERIFY_TMP/recovery"
RECOVERY_CWD="$RECOVERY_ROOT/work"
RECOVERY_MEDIA="$RECOVERY_CWD/media"
RECOVERY_OUT="$RECOVERY_CWD/out"
RECOVERY_STATE="$RECOVERY_ROOT/state"
RECOVERY_DATA="$RECOVERY_ROOT/data"
RECOVERY_CACHE="$RECOVERY_ROOT/cache"
mkdir -p "$RECOVERY_MEDIA" "$RECOVERY_STATE" "$RECOVERY_DATA" "$RECOVERY_CACHE"
cp "$SPEECH_MP3" "$RECOVERY_MEDIA/01-complete.mp3"
cp "$SPEECH_MP3" "$RECOVERY_ROOT/02-retry.mp3"
cp "$SPEECH_MP3" "$RECOVERY_MEDIA/02-retry.mp3"

recovery_cue() {
  (
    cd "$RECOVERY_CWD" || exit 1
    env "CUE_CONFIG_DIR=$CFG_DIR" \
      "CUE_STATE_DIR=$RECOVERY_STATE" \
      "CUE_DATA_DIR=$RECOVERY_DATA" \
      "CUE_CACHE_DIR=$RECOVERY_CACHE" \
      "$CUE" "$@"
  )
}

check "isolated recovery worker provisioned" recovery_cue doctor --fix

run_mixed_recovery_failure() {
  (
    cd "$RECOVERY_CWD" || exit 1
    (
      attempts=0
      while ! find "$RECOVERY_STATE" -type f -name '*.json' -print -quit 2>/dev/null | grep -q .; do
        attempts=$((attempts+1))
        [ "$attempts" -lt 400 ] || exit 2
        sleep 0.05
      done
      rm "$RECOVERY_MEDIA/02-retry.mp3"
    ) &
    remover=$!
    env "CUE_CONFIG_DIR=$CFG_DIR" \
      "CUE_STATE_DIR=$RECOVERY_STATE" \
      "CUE_DATA_DIR=$RECOVERY_DATA" \
      "CUE_CACHE_DIR=$RECOVERY_CACHE" \
      "$CUE" media --output "$RECOVERY_OUT"
    cue_status=$?
    wait "$remover"
    remover_status=$?
    [ "$remover_status" -eq 0 ] || return "$remover_status"
    return "$cue_status"
  )
}

check_fail "mixed batch records a repaired retry" run_mixed_recovery_failure
check "recovery journal created after preflight" bash -c "find '$RECOVERY_STATE' -type f -name '*.json' -print -quit | grep -q ."
RECOVERY_JOURNAL="$(find "$RECOVERY_STATE" -type f -name '*.json' -print -quit)"
RECOVERY_BATCH_ID="$(python3 -c "import json; print(json.load(open('$RECOVERY_JOURNAL'))['id'])")"
RECOVERY_FIRST_WORKSPACE="$(python3 -c "import json; print(json.load(open('$RECOVERY_JOURNAL'))['items'][0]['workspace'])")"
RECOVERY_SECOND_WORKSPACE="$(python3 -c "import json; print(json.load(open('$RECOVERY_JOURNAL'))['items'][1]['workspace'])")"
RECOVERY_FIRST_RECEIPT="$RECOVERY_FIRST_WORKSPACE/cue.run.json"

check "batches list exposes incomplete ID" bash -c "recovery_output=\"\$(cd '$RECOVERY_CWD' && env CUE_STATE_DIR='$RECOVERY_STATE' '$CUE' batches list)\"; printf '%s\n' \"\$recovery_output\" | grep -q '$RECOVERY_BATCH_ID.*incomplete.*1/2 complete'"
check "batches show exposes missing retry" bash -c "cd '$RECOVERY_CWD' && env CUE_STATE_DIR='$RECOVERY_STATE' '$CUE' batches show '$RECOVERY_BATCH_ID' | grep -q '02-retry.mp3.*missing'"
check "first batch receipt is attempt-bound schema v3" bash -c "python3 -c \"import json; d=json.load(open('$RECOVERY_FIRST_RECEIPT')); assert d['schema_version']==3; assert d['batch_attempt']['batch_id']=='$RECOVERY_BATCH_ID'; assert d['batch_attempt']['item_position']==0; assert d['batch_attempt']['attempt_number']==1\""
touch -t 200001010000 "$RECOVERY_FIRST_RECEIPT"
RECOVERY_FIRST_MTIME="$(mtime_ns "$RECOVERY_FIRST_RECEIPT")"

cp "$RECOVERY_ROOT/02-retry.mp3" "$RECOVERY_MEDIA/02-retry.mp3"
cp "$SPEECH_MP3" "$RECOVERY_MEDIA/03-added-later.mp3"
check "default resume repairs only original membership" recovery_cue resume
check "verified prior success is not regenerated" mtime_is_unchanged "$RECOVERY_FIRST_RECEIPT" "$RECOVERY_FIRST_MTIME"
check "failed original receives a second attempt" bash -c "python3 -c \"import json; d=json.load(open('$RECOVERY_SECOND_WORKSPACE/cue.run.json')); assert d['schema_version']==3; assert d['batch_attempt']['batch_id']=='$RECOVERY_BATCH_ID'; assert d['batch_attempt']['item_position']==1; assert d['batch_attempt']['attempt_number']==2\""
check_fail "new directory media is excluded from frozen batch" test -e "$RECOVERY_OUT/03-added-later.srt"
check_fail "new directory media gets no hidden workspace" test -e "$RECOVERY_OUT/.cue/03-added-later"
check "batches show reports complete" bash -c "cd '$RECOVERY_CWD' && env CUE_STATE_DIR='$RECOVERY_STATE' '$CUE' batches show '$RECOVERY_BATCH_ID' | grep -q '^Status: complete$'"
check "batches list reports 2/2 complete" bash -c "cd '$RECOVERY_CWD' && env CUE_STATE_DIR='$RECOVERY_STATE' '$CUE' batches list | grep -q '$RECOVERY_BATCH_ID.*complete.*2/2 complete'"
check "transcript-only recovery dispatch seam" bash -c "cd '$ROOT' && cargo test -q -p cue-cli --test batches resume_dispatches_both_recorded_processing_modes"

echo
echo "== skill =="
check "skill help"         "$CUE" skill --help
check "skill smoke script"  bash -n scripts/test_skill_install.sh
check "SKILL.md frontmatter" bash -c "grep -q '^name: transcribe' skills/transcribe/SKILL.md && grep -q '^description:' skills/transcribe/SKILL.md"
check "evals.json valid"   bash -c "python3 -c \"import json; d=json.load(open('skills/transcribe/evals/evals.json')); assert len(d['evals'])>=2; assert all(c['assertions'] for c in d['evals'])\""
check "README recovery contract" bash -c "grep -q 'cue resume \[ID-OR-PATH\]' README.md && grep -q 'CUE_STATE_DIR' README.md && grep -q 'verify --json.*schema version 2' README.md"
check "skill recovery contract" bash -c "grep -q 'cue 0.13.0 or newer' skills/transcribe/SKILL.md && grep -q 'cue batches show <ID-OR-PATH>' skills/transcribe/SKILL.md && grep -q 'Never hand-edit a recovery journal' skills/transcribe/SKILL.md"
check_fail "no real identifiers" bash -c "grep -riE 'eastham|dometrain' skills/transcribe/ || exit 1; exit 0"

echo
echo "== correct command =="
VERIFY_DIR=$(mktemp -d "$VERIFY_TMP/correct.XXXXXX")
mkdir -p "$VERIFY_DIR/talk.cue"
printf 'I am John Dough. See open telemetry.\n' > "$VERIFY_DIR/talk.cue/transcript.txt"
printf 'open telemetry is key.\n' > "$VERIFY_DIR/talk.cue/subtitles.srt"
cat > "$VERIFY_DIR/talk.cue/transcript.json" <<'EOF'
{
  "schema_version": 1,
  "language": "en",
  "duration_ms": 1600,
  "words": [
    {"text":"I","start_ms":0,"end_ms":100,"confidence":0.9,"speaker":null},
    {"text":"am","start_ms":110,"end_ms":250,"confidence":0.9,"speaker":null},
    {"text":"John","start_ms":260,"end_ms":450,"confidence":0.9,"speaker":null},
    {"text":"Dough.","start_ms":460,"end_ms":700,"confidence":0.9,"speaker":null},
    {"text":"See","start_ms":800,"end_ms":950,"confidence":0.9,"speaker":null},
    {"text":"open","start_ms":960,"end_ms":1150,"confidence":0.9,"speaker":null},
    {"text":"telemetry.","start_ms":1160,"end_ms":1600,"confidence":0.9,"speaker":null}
  ],
  "segments": [
    {"start_ms":0,"end_ms":1600,"text":"I am John Dough. See open telemetry.","word_start":0,"word_end":7}
  ]
}
EOF
printf '{"analysis":"UNCHANGED"}\n' > "$VERIFY_DIR/talk.cue/analysis.json"
printf '{"schema_version":1,"chunks":[{"start_ms":0,"end_ms":1600,"text":"UNCHANGED"}]}\n' > "$VERIFY_DIR/talk.cue/normalized.json"
printf 'STALE RECEIPT\n' > "$VERIFY_DIR/talk.cue/corrections.applied.json"
printf '{}\n' > "$VERIFY_DIR/talk.cue/cue.run.json"
printf 'John Dough -> John Doe\nopen telemetry -> OpenTelemetry\n' > "$VERIFY_DIR/corrections.md"
BEFORE=$(shasum -a 256 "$VERIFY_DIR/talk.cue/transcript.txt" "$VERIFY_DIR/talk.cue/subtitles.srt" "$VERIFY_DIR/talk.cue/transcript.json" "$VERIFY_DIR/talk.cue/normalized.json" "$VERIFY_DIR/talk.cue/analysis.json" "$VERIFY_DIR/talk.cue/corrections.applied.json" | shasum | cut -d' ' -f1)
check "correct dry-run writes nothing" bash -c "$CUE correct $VERIFY_DIR/talk.cue --dry-run 2>&1 | grep -q 'Dry run'"
AFTER=$(shasum -a 256 "$VERIFY_DIR/talk.cue/transcript.txt" "$VERIFY_DIR/talk.cue/subtitles.srt" "$VERIFY_DIR/talk.cue/transcript.json" "$VERIFY_DIR/talk.cue/normalized.json" "$VERIFY_DIR/talk.cue/analysis.json" "$VERIFY_DIR/talk.cue/corrections.applied.json" | shasum | cut -d' ' -f1)
check "correct dry-run leaves all artifacts unchanged" test "$BEFORE" = "$AFTER"
check "correct dry-run keeps run receipt" test -s "$VERIFY_DIR/talk.cue/cue.run.json"
check "correct applies"               bash -c "$CUE correct $VERIFY_DIR/talk.cue 2>&1 | grep -q 'replacement(s)'"
check_fail "correct invalidates run receipt" test -e "$VERIFY_DIR/talk.cue/cue.run.json"
check "correct fixed transcript"      bash -c "grep -q 'OpenTelemetry' $VERIFY_DIR/talk.cue/transcript.txt && ! grep -q 'John Dough' $VERIFY_DIR/talk.cue/transcript.txt"
check "correct fixed subtitles"       bash -c "grep -q 'OpenTelemetry' $VERIFY_DIR/talk.cue/subtitles.srt"
check "correct wrote receipt"          bash -c "python3 -c \"import json; d=json.load(open('$VERIFY_DIR/talk.cue/corrections.applied.json')); assert d['schema_version']==2; assert len(d['manifests'])==1; assert len(d['rules'])==2\""
check "correct kept canonical json"   bash -c "grep -q 'John Dough' $VERIFY_DIR/talk.cue/transcript.json"
check "correct kept normalized"        bash -c "grep -q 'UNCHANGED' '$VERIFY_DIR/talk.cue/normalized.json'"
check "correct kept analysis"         bash -c "grep -q 'UNCHANGED' $VERIFY_DIR/talk.cue/analysis.json"
check "review emits versioned JSON" bash -c "$CUE review '$VERIFY_DIR/talk.cue' --json | python3 -c \"import json,sys; d=json.load(sys.stdin); assert d['schema_version']==2; assert isinstance(d['diagnostics'], list)\""
mkdir "$VERIFY_DIR/promoted"
check "promote verified correction" "$CUE" lexicon promote "$VERIFY_DIR/talk.cue" --rule "open telemetry" --to "$VERIFY_DIR/promoted"
check "promoted lexicon contains rule" grep -q "open telemetry -> OpenTelemetry" "$VERIFY_DIR/promoted/corrections.md"
check "promotion emits JSON attestation" bash -c "$CUE lexicon promote '$VERIFY_DIR/talk.cue' --rule 'open telemetry' --to '$VERIFY_DIR/promoted' --json | python3 -c \"import json,sys; d=json.load(sys.stdin); assert d['schema_version']==1; assert d['status']=='already-present'; assert len(d['source_receipt_hash'])==64; assert len(d['target_lexicon_hash'])==64\""

kill $GW 2>/dev/null || true

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "ALL CHECKS PASSED"
else
  echo "$FAILURES CHECK(S) FAILED"
fi
exit $FAILURES
