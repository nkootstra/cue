# cue

Turn local video and audio files into accurate transcripts, subtitles, and
AI-generated descriptions.

```bash
cue ./video.mp4
```

produces a `video.cue/` directory containing transcripts (`json`, `txt`,
cleaned `txt`), subtitles (`srt`, `vtt`) and — when an LLM gateway is
configured — summaries, descriptions, and chapters.

## Status

Work in progress. Current milestone: **foundation**.

- [x] Workspace, CLI surface (`usage-rs`), configuration layering
- [x] `cue doctor` environment checks (FFmpeg, FFprobe, Python)
- [x] Media inspection via ffprobe
- [ ] Local transcription (faster-whisper worker)
- [ ] Subtitle generation (SRT/WebVTT)
- [ ] S1-mini normalization via Ollama
- [ ] LLM analysis (OpenAI-compatible gateways: Ollama `/v1`, OpenRouter)
- [ ] Content-addressed caching and resumable pipeline

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
```

Requirements: FFmpeg/FFprobe on PATH, Python 3.10+ for the transcription
worker (auto-provisioned later by `cue doctor --fix`), optionally Ollama for
S1 normalization.
