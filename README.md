# cue

Turn local video and audio files into accurate transcripts, subtitles, and
AI-generated descriptions.

```bash
cue ./video.mp4
```

publishes `video.srt` beside the media. Canonical transcripts, receipts, and
optional analysis remain available in the hidden `.cue/video/` workspace.

## Why cue?

- **Local-first privacy.** Media, extracted audio, the raw transcript, and
  subtitles stay on your machine. S1 normalization runs through your local
  Ollama instance. When analysis is enabled, cue sends only the normalized
  transcript text to the explicitly configured LLM gateway; a remote gateway
  such as OpenRouter receives that text, while a local Ollama gateway does not
  send it off-machine.
- **Canonical transcript as source of truth.** The raw timed transcript
  (`transcript.json`) is never overwritten. Cleaned text, subtitles, and
  analysis all derive from it.
- **Deterministic corrections.** Fix misheard names and terms by writing a
  plain-text corrections manifest. cue applies it during normal processing
  and cached reruns, never corrupting the raw archive.
- **Reviewable vocabulary reuse.** Compose project, course, folder, and
  source lexicons, inspect focused correction candidates, and explicitly
  promote only corrections that a completed render proved useful.
- **Fast reruns.** Content-addressed caching means a rerun skips completed
  extraction, transcription, normalization, and analysis.
- **Replaceable providers.** faster-whisper, S1, and OpenAI-compatible
  gateways sit behind traits, so you can swap local models or gateways
  without changing how you work.
- **Self-contained.** One binary that embeds and auto-provisions its Python
  worker (`cue doctor --fix`); no manual environment setup.
- **Agent-ready.** Ships a `transcribe` agent skill with an eval suite, so
  coding agents can transcribe and correct media for you.
- **Word-level timestamps.** Accurate per-word timing powers subtitles and
  timestamped analysis.

## Installation

**Homebrew** (macOS and Linux):

```bash
brew install nkootstra/tap/cue
```

**Install script** (macOS and Linux, no Rust required):

```bash
curl -fsSL https://raw.githubusercontent.com/nkootstra/cue/main/install.sh | sh
```

Installs to `~/.local/bin` (override with `CUE_INSTALL_DIR`); pin a release
with `--version vX.Y.Z`.

**With Rust toolchain:**

```bash
cargo install --git https://github.com/nkootstra/cue cue
```

The binary is self-contained — the Python transcription worker and its
pinned dependencies are embedded and provisioned on demand. Runtime
requirements: FFmpeg on PATH (`brew install ffmpeg` / `apt install ffmpeg`);
Ollama optional, for S1 transcript cleanup. After installing, run
`cue doctor` to check your environment.

## Commands

| Command | What it does |
|---|---|
| `cue <path>...` | Process one or more media files or directories through the full pipeline |
| `cue transcribe <path>...` | Process paths through the canonical transcript only (no subtitles/analysis) |
| `cue correct <file>.cue` | Rebuild an existing output from canonical JSON and apply corrections |
| `cue review <file>.cue [--json]` | Report focused correction candidates without changing files |
| `cue verify <file>.cue [--json]` | Verify a completed output against its source, corrections, and artifact hashes |
| `cue lexicon promote <file>.cue --rule <phrase> --to <dir>` | Promote a verified applied rule into a reusable scope |
| `cue subtitles check <file>.cue [--json]` | Check generated subtitle cues and report source-linked policy findings |
| `cue doctor` | Check required and optional tools; `--fix` provisions the Python worker |
| `cue models list/check/install s1` | Manage transcription/normalization models in Ollama |
| `cue skill install [--local]` | Install the `transcribe` agent skill globally, or in the current project |
| `cue config` | Show resolved configuration and its sources |
| `cue cache dir/clear` | Inspect or clear the content-addressed cache |

Use `--format vtt` to replace the default SRT output, or repeat `--format` to
publish both formats. Use `--summary` to print a content summary to stdout;
add `--stream` for completion-order summaries in concurrent batches.

## Multiple files and directories

Both the default pipeline and `cue transcribe` accept more than one path:

```bash
cue intro.mp4 interview.wav
cue transcribe intro.mp4 interview.wav
```

Directories are scanned at their top level by default. Add `--recursive` to
include nested directories:

```bash
cue ./course
cue --recursive ./course
cue --recursive ./course --output ./transcripts
cue --recursive ./course --jobs 2
```

Directory discovery recognizes these extensions, case-insensitively:
`aac`, `aif`, `aiff`, `flac`, `m4a`, `mp3`, `ogg`, `opus`, `wav`, `avi`,
`m2ts`, `m4v`, `mkv`, `mov`, `mp4`, `mpeg`, `mpg`, `mts`, `ts`, and `webm`.
An explicitly named file is still inspected by FFprobe regardless of its
extension.

Each discovered file publishes one `<stem>.srt` sidecar by default and keeps
supporting state in `<source-dir>/.cue/<stem>/`. `--output <dir>` redirects
both: visible subtitles retain their relative source path beneath the output
root, and workspaces live beneath `<output>/.cue/`. Explicit file lists use
their stems at the output root. Existing `<stem>.cue/` workspaces are reused;
if both legacy and hidden workspaces exist, cue stops rather than guessing.

cue preserves the order of positional path groups and sorts files within each
directory lexically. It removes canonical duplicates, includes hidden media
files, and never traverses directory symlinks; pass the real directory instead
of an explicit directory symlink. Destination-name collisions are rejected
before processing begins. Batch files are processed sequentially by default.
Set `--jobs <N>` to process at most N files concurrently; start conservatively
because transcription and local model stages can be memory intensive. The
`--jobs` option requires cue 0.10.0 or newer; upgrade an older binary if it
rejects the option. TTY output shows one progress row per file, while piped
output uses source-labelled messages. Each file keeps its own output receipt.
One file's failure does not stop the rest; failures are reported in input order,
the final summary reports successes and failures, and any failure produces exit
status 1. Inputs with identical content safely share cached work without writing
that cache concurrently.

Processing a source again regenerates its derived text and subtitle artifacts.
When a corrections manifest remains discoverable, cue reapplies it during
that render, including fully cached reruns.

## Outputs

`cue ./video.mp4` publishes `video.srt` and writes supporting state into the
hidden `.cue/video/` workspace:

| File | Role |
|---|---|
| `transcript.json` | Raw canonical transcript (timed words) — the source of truth, never edited |
| `transcript.txt` | Plain text derived from the canonical transcript |
| `transcript.clean.txt` / `normalized.json` | S1-cleaned prose (when Ollama has S1) |
| `subtitles.srt` / `subtitles.vtt` | Workspace copies of explicitly selected subtitle formats |
| `analysis.json` / `summary.md` / `description.md` | LLM analysis (when a gateway is configured) |
| `corrections.md` | Optional manifest you write to fix misheard names/terms |
| `corrections.applied.json` | Versioned receipt recording contributing lexicons, canonical source hashes, rule provenance, and per-artifact applications |
| `cue.run.json` | Completion receipt recording the source, effective configuration, providers, stage/cache outcomes, warnings, remote-data use, corrections, and final artifact hashes |

The visible sidecar is also recorded in `cue.run.json`. To publish VTT instead
of SRT, run `cue --format vtt ./video.mp4`. Repeat `--format` to request more
than one format explicitly.

### Terminal summaries

Summaries are opt-in and require local S1 normalization plus a configured
analysis gateway:

```bash
cue --summary ./video.mp4
cue --summary ./course --recursive --jobs 2
cue --summary --stream ./course --recursive --jobs 2
```

Summary blocks go to stdout and are source-labelled. Progress, diagnostics,
and the batch result go to stderr. The default is deterministic input order;
`--stream` emits each complete block as its file finishes. It never streams
partial model tokens and does not create an additional visible summary file.

## Verifying completed outputs

A successful render publishes `cue.run.json` last, so its presence marks a
completed run rather than a partially written output. Verify the source,
correction manifests, and every recorded artifact at any time:

```bash
cue verify video.mp4
cue verify video.mp4 --json
```

Exit status 0 means all recorded files still match. Exit status 1 means a file
is missing, unreadable, or has changed, or the receipt itself cannot be read.
The JSON form provides stable diagnostic IDs for automation. The receipt never
contains API keys; URL credentials, query strings, and fragments are removed
from recorded provider endpoints. Remote-data metadata describes only transfers
performed by the current run; cached artifacts do not claim the provider
lineage of the run that originally created them.

The receipt is an integrity record, not a cryptographic signature: it proves
that the recorded files still agree with one another, but not who produced or
approved them. Only trust receipts from a source you trust.

`cue correct` deliberately invalidates `cue.run.json` before changing derived
files because the old run no longer attests to them. Rerun the source with its
correction manifest (cached stages are reused) to publish a new receipt after
the corrections have been applied.

## Correcting transcripts

Speech-to-text mishears names and technical terms. Instead of editing files
by hand, write a corrections manifest and let cue apply it deterministically
to every text artifact:

```text
# corrections.md
John Dough -> John Doe
open telemetry -> OpenTelemetry
```

```bash
cue video.mp4 --corrections corrections.md                    # process and apply
cue correct video.cue --corrections corrections.md --dry-run  # preview
cue correct video.cue --corrections corrections.md            # rebuild and apply
```

The global `--corrections` flag works with the default pipeline,
`cue transcribe`, `cue correct`, and `cue review`. Passing it uses only that
explicit manifest. Without it, cue composes scoped `corrections.md` files.
For an output beneath the current working directory, discovery starts at the
working directory and continues down through its ancestor folders to the
output itself. Broader rules load first; a nearer scope wins when two files
map the same phrase, while unrelated rules from every scope remain active.
For an output outside the current working directory, discovery is deliberately
limited to the output directory and its direct parent so cue never searches
arbitrary filesystem ancestors.

Normal processing applies the effective lexicon after rendering, so cached
reruns keep corrections. Each corrected render writes a schema-v2
`corrections.applied.json` containing every contributing manifest, its hash,
the winning source for each effective rule, and per-artifact application
counts. The receipt proves a render; it is not a source of truth. Change a
manifest and rerun to replace the old corrections from canonical data. Remove
all applicable manifests and rerun to restore uncorrected derived files and
remove the stale receipt.

`cue correct` follows the same canonical model for an existing output: it
reconstructs `transcript.txt`, cleaned text, and SRT/VTT files from
`transcript.json` and `normalized.json`, then applies the manifest. Manual
edits to those derived files are therefore overwritten. Corrections never
modify `transcript.json`, `normalized.json`, or existing analysis outputs.
Matching is case-insensitive and whole-phrase, so `dough` won't affect
`doughnut`.

### Review and promote corrections

Use the focused review command instead of rereading an entire transcript:

```bash
cue review video.cue
cue review video.cue --confidence-below 0.60 --json
```

The report is read-only. It identifies low-confidence words, words that may
have segment-level fallback timing, effective rules that do not match the
canonical transcript, conflicting scoped rules, and ambiguous speaker turns
when speaker assignments exist. Diagnostic IDs are stable; `--json` emits a
versioned report suitable for agents and scripts. Candidates are evidence to
review, not automatic corrections—the correct spelling still requires known
context.

After applying and verifying a source-specific correction, promote it into an
existing project, course, or folder directory so later files reuse it:

```bash
cue lexicon promote video.cue \
  --rule "open telemetry" \
  --to /path/to/course
```

Promotion reads `corrections.applied.json` and succeeds only when its manifest
and canonical-source hashes still match the current files and the selected
rule made at least one replacement. It appends the verified mapping to the
target directory's `corrections.md`, is idempotent when the same mapping is
already present, and refuses to overwrite a conflicting mapping. The target
directory must be explicit and already exist. Cue never promotes or “learns”
corrections without this command.
Add `--json` when a workflow needs a machine-readable attestation bound to the
source receipt and resulting target lexicon.

### Check subtitle quality

Check the cues generated from the canonical transcript without changing any
output files:

```bash
cue subtitles check video.cue
cue subtitles check video.cue --json
```

The check reports cues that exceed the generic duration, line-capacity, or
reading-speed policy. It also exposes any overlap shortening or dropped cue
that cue applied to keep subtitle timing monotonic. Every diagnostic includes
the half-open canonical word range (`word_start..word_end`) that produced it,
so the source can be inspected without guessing from rendered subtitle text.

No findings exits with status 0. Findings or an operational error exit with
status 1, which makes the command suitable for local scripts and CI gates.
`--json` emits a versioned report with stable diagnostic IDs. The current
policy is `cue-generic-v1`; configured line and duration limits are included
in the report, while the generic reading-speed limit is 20 characters per
second.

## Agent skill

AI coding agents (Claude Code, opencode, Codex, Cursor, ...) can transcribe
media and correct misheard names and terminology with context using the
bundled `transcribe` skill. Install it for your agents:

```bash
cue skill install          # global; available across projects
cue skill install --local  # current project only
# or, without cue installed:
npx --yes skills@1.5.9 add nkootstra/cue --global -y
```

The skill teaches agents to install cue when missing, run the pipeline,
gather speaker/domain context, review focused candidates, write scoped
correction manifests, apply them during the main cue invocation or a safe
cached rerun, and explicitly promote verified rules when the user approves a
target scope. It also teaches agents to respect the privacy boundary:
media, audio, raw
transcripts, and subtitles stay local; only normalized text may be sent to
the configured analysis gateway. All examples in the skill use fictional
identities.

The skill lives in `skills/transcribe/` and ships with an eval suite
(`skills/transcribe/evals/evals.json`). Run the eval harness to regenerate
fixtures and re-run the correction cases:

```bash
eval_workspace="$(mktemp -d "${TMPDIR:-/tmp}/cue-transcribe-eval.XXXXXX")"
scripts/skill-eval-harness.sh --seed "$eval_workspace"
# Run the printed agent prompts against this workspace, then grade the results.
scripts/skill-eval-harness.sh --grade "$eval_workspace"
```

## Status

Work in progress.

- [x] Workspace, CLI surface (`usage-rs`), configuration layering
- [x] `cue doctor` environment checks (FFmpeg, FFprobe, Python)
- [x] Media inspection via ffprobe
- [x] Local transcription (faster-whisper worker, auto-provisioned venv)
- [x] Subtitle generation (SRT/WebVTT)
- [x] S1-mini normalization via Ollama
- [x] LLM analysis (OpenAI-compatible gateways: Ollama `/v1`, OpenRouter)
- [x] Content-addressed caching for every stage; reruns skip completed work
- [x] Pipeline event bus with TTY-aware rendering
- [x] Durable scoped correction lexicons, focused review, explicit promotion,
      and application receipts
- [x] Source-linked subtitle policy checks with human and JSON diagnostics
- [x] Attested run receipts and source/artifact verification
- [x] `transcribe` agent skill with evals
- [x] Incremental progress within stages (worker-level reporting)
- [x] Bounded parallel file processing with `--jobs`

## Design principles

**Local first.** Media, extracted audio, raw transcripts, subtitles, and S1
normalization stay local. Analysis sends normalized transcript text to the
configured LLM gateway. Choose a local Ollama gateway to keep that text on
the machine; choosing a remote gateway such as OpenRouter sends it to that
provider.

**Canonical transcript.** The raw timed transcript is the source of truth.
Cleaned text is derived and never replaces it.

**Replaceable providers.** Transcription, normalization, and analysis each
sit behind internal traits, so faster-whisper, S1, Ollama, OpenRouter, and
friends are implementation details rather than load-bearing dependencies.

## Configuration

User config lives at `~/.config/cue/cue.toml`. Precedence:
CLI arguments > environment > user config > defaults.

```toml
[transcription]
provider = "faster-whisper"
model = "large-v3-turbo"

[normalization]
provider = "s1"
ollama_url = "http://localhost:11434"         # local endpoints only

[llm]
base_url = "https://openrouter.ai/api/v1"   # or http://localhost:11434/v1
model = "model-name"
api_key_env = "CUE_LLM_API_KEY"             # key read from env, never stored
```

S1 normalization is deliberately restricted to `localhost` and IP loopback
addresses. Remote OpenAI-compatible endpoints remain supported for analysis
through `llm.base_url`.

## Development

```bash
cargo test          # unit + integration tests
cargo run -- doctor # check the local environment
scripts/verify.sh   # full gate: fmt, build, test, clippy + end-to-end battery
```

`scripts/verify.sh` builds a speech fixture with macOS `say`, runs the whole
pipeline against a local mock gateway, and asserts on every output artifact.

Requirements: FFmpeg/FFprobe on PATH, Python 3.10+ for the transcription
worker (auto-provisioned later by `cue doctor --fix`), optionally Ollama for
S1 normalization.
