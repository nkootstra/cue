# cue

Turn local video and audio files into accurate transcripts, subtitles, and
AI-generated descriptions.

```bash
cue ./video.mp4
```

produces a `video.cue/` directory containing transcripts (`json`, `txt`,
cleaned `txt`), subtitles (`srt`, `vtt`) and — when an LLM gateway is
configured — summaries, descriptions, and chapters.

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
| `cue doctor` | Check required and optional tools; `--fix` provisions the Python worker |
| `cue models list/check/install s1` | Manage transcription/normalization models in Ollama |
| `cue skill install [--local]` | Install the `transcribe` agent skill globally, or in the current project |
| `cue config` | Show resolved configuration and its sources |
| `cue cache dir/clear` | Inspect or clear the content-addressed cache |

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
```

Directory discovery recognizes these extensions, case-insensitively:
`aac`, `aif`, `aiff`, `flac`, `m4a`, `mp3`, `ogg`, `opus`, `wav`, `avi`,
`m2ts`, `m4v`, `mkv`, `mov`, `mp4`, `mpeg`, `mpg`, `mts`, `ts`, and `webm`.
An explicitly named file is still inspected by FFprobe regardless of its
extension.

Each discovered file gets its own `<stem>.cue/` output directory. Without
`--output`, that directory is created beside the source. In a batch,
`--output <dir>` makes `<dir>` the common root, for example
`transcripts/intro.cue/`. A single explicit file keeps the existing behavior:
`--output` names its output directory directly.

cue preserves the order of positional path groups and sorts files within each
directory lexically. It removes canonical duplicates, includes hidden media
files, and never traverses directory symlinks; pass the real directory instead
of an explicit directory symlink. Destination-name collisions are rejected
before processing begins. Batch files are processed sequentially, and one
file's failure does not stop the rest; the final summary reports successes and
failures, and any failure produces exit status 1.

Processing a source again regenerates its derived text and subtitle artifacts.
When a corrections manifest remains discoverable, cue reapplies it during
that render, including fully cached reruns.

## Outputs

`cue ./video.mp4` writes into `video.cue/`:

| File | Role |
|---|---|
| `transcript.json` | Raw canonical transcript (timed words) — the source of truth, never edited |
| `transcript.txt` | Plain text derived from the canonical transcript |
| `transcript.clean.txt` / `normalized.json` | S1-cleaned prose (when Ollama has S1) |
| `subtitles.srt` / `subtitles.vtt` | Subtitles from the canonical transcript |
| `analysis.json` / `summary.md` / `description.md` | LLM analysis (when a gateway is configured) |
| `corrections.md` | Optional manifest you write to fix misheard names/terms |
| `corrections.applied.json` | Versioned receipt written when corrections are applied, recording source hashes and per-rule applications |

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
`cue transcribe`, and `cue correct`. Without it, cue looks for
`corrections.md` in the output directory and then its parent. The explicit
flag takes precedence, followed by the output-directory manifest and the
parent-directory manifest.

Normal processing applies the selected manifest after rendering, so cached
reruns keep corrections. Each corrected render writes
`corrections.applied.json`; this receipt records what was applied but is not a
source of truth. Change the manifest and rerun to replace the old corrections
from canonical data. Remove the manifest and rerun to restore uncorrected
derived files and remove the stale receipt.

`cue correct` follows the same canonical model for an existing output: it
reconstructs `transcript.txt`, cleaned text, and SRT/VTT files from
`transcript.json` and `normalized.json`, then applies the manifest. Manual
edits to those derived files are therefore overwritten. Corrections never
modify `transcript.json`, `normalized.json`, or existing analysis outputs.
Matching is case-insensitive and whole-phrase, so `dough` won't affect
`doughnut`.

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
gather speaker/domain context, write a corrections manifest, apply it during
the main cue invocation or a safe cached rerun, and use `cue correct` for
existing outputs. It also teaches agents to respect the privacy boundary:
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
- [x] Durable deterministic transcript corrections and application receipts
- [x] `transcribe` agent skill with evals
- [ ] Incremental progress within stages (worker-level reporting)
- [ ] Parallel file processing

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
