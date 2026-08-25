# cue

Turn local video and audio files into accurate transcripts, subtitles, and
AI-generated descriptions.

```bash
cue ./video.mp4
```

produces a `video.cue/` directory containing transcripts (`json`, `txt`,
cleaned `txt`), subtitles (`srt`, `vtt`) and — when an LLM gateway is
configured — summaries, descriptions, and chapters.

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

## Agent skill

AI coding agents (Claude Code, opencode, Codex, Cursor, ...) can transcribe
media and correct misheard names and terminology with context using the
bundled `transcribe` skill. Install it for your agents:

```bash
cue skill install          # proxies `npx skills add nkootstra/cue`
# or, without cue installed:
npx skills add nkootstra/cue
```

The skill teaches agents to install cue when missing, run the pipeline,
correct speech-to-text errors using speaker/domain context (e.g. a surname
whisper misheard, fixed from a context file), batch-process course folders,
and respect the privacy boundary (media stays local). All examples in the
skill use fictional identities.

The skill lives in `skills/transcribe/` and ships with an eval suite
(`skills/transcribe/evals/evals.json`). Run the eval harness to regenerate
fixtures and re-run the correction cases:

```bash
scripts/skill-eval-harness.sh      # fixtures + cue runs + agent prompts
scripts/skill-eval-harness.sh --grade
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
- [ ] Incremental progress within stages (worker-level reporting)
- [ ] Parallel file processing

## Design principles

**Local first.** Media, transcription, and normalization never leave the
machine. Only normalized transcript text may be sent to a configured LLM
gateway for analysis.

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
ollama_url = "http://localhost:11434"

[llm]
base_url = "https://openrouter.ai/api/v1"   # or http://localhost:11434/v1
model = "model-name"
api_key_env = "CUE_LLM_API_KEY"             # key read from env, never stored
```

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
