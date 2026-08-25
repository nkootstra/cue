---
name: transcribe
description: Transcribe video and audio files into accurate transcripts, subtitles, and descriptions using cue. Use when the user wants to transcribe media, generate subtitles or captions, extract spoken content from recordings, batch-process a course or folder of videos, or clean up transcriptions where names, terms, or technical vocabulary were misheard. Installs cue when it is missing and corrects transcription errors using context about the speaker, repository, or domain.
---

# Transcribe media with cue

Turn local video/audio into transcripts, subtitles, and (optionally)
AI-generated descriptions, then correct misheard names and terminology
using context.

## When to use

- The user points at a video or audio file and wants its spoken content.
- The user wants subtitles (SRT/VTT) for a video.
- The user wants a batch of media transcribed (a course, a lecture series, a
  folder of recordings).
- The transcript exists but contains obvious mishearings (names, jargon,
  technical terms) that context can fix.

## 1. Make sure cue is installed

Check whether cue exists before doing anything else:

```bash
command -v cue
```

If it is missing, install it. Prefer in this order:

1. Homebrew (macOS/Linux): `brew install nkootstra/tap/cue`
2. Install script (no Rust needed): `curl -fsSL https://raw.githubusercontent.com/nkootstra/cue/main/install.sh | sh`
3. Cargo: `cargo install --git https://github.com/nkootstra/cue cue`

After installing, verify the environment:

```bash
cue doctor
```

If the required tools are fine but the Python worker is missing, provision it:

```bash
cue doctor --fix
```

`cue doctor` reports optional integrations (Ollama, S1, an LLM gateway)
separately — those are not required for transcription.

## 2. Transcribe a file

```bash
cue <file.mp4> [--language en] [--output <dir>]
```

cue runs the whole local pipeline: inspect -> extract audio -> transcribe ->
normalize (S1, if installed) -> analyze (if a gateway is configured) ->
write outputs. Outputs land in `<file>.cue/` next to the source (or `--output`):

| File | Role |
|---|---|
| `transcript.json` | **Raw canonical transcript.** Timed words, the source of truth. Never edit this. |
| `transcript.txt` | Plain text derived from the canonical transcript. This is the file you correct. |
| `normalized.json` / `transcript.clean.txt` | S1-cleaned prose (present when Ollama has the S1 model). |
| `subtitles.srt` / `subtitles.vtt` | Subtitles derived from the canonical transcript. |
| `analysis.json` / `summary.md` / `description.md` | LLM analysis (present when a gateway is configured). |

If the user only wants the plain transcript (no subtitles/analysis):

```bash
cue transcribe <file.mp4>
```

## 3. Correct mishearings with context

Speech-to-text reliably mishears proper nouns (a surname heard as a
similarly-pronounced word), technical terms, and domain jargon. The audio
itself is ambiguous, so the fix comes from **context** you already have or
can gather.

### Gather context

In this order, until you have enough:

1. **The user.** Ask who is speaking and what the material is about. Names
   and product names matter most.
2. **The repository/project.** README, package metadata, docs, contributor
   names — often identify the speaker and the domain vocabulary.
3. **The file/folder itself.** A path like
   `Talks/acme-dev-conf-2025/intro-to-observability/01-opening.mp4` tells
   you the platform, the talk, and the topic. Filenames of sibling files
   carry speaker names and series info.
4. **Inferred domain vocabulary.** For a Rust course: cargo, crates,
   borrow checker, lifetimes. For a conference talk: the conference name,
   the talk title. For an observability talk: OpenTelemetry, spans,
   metrics, traces.

### Apply corrections

1. Read `transcript.txt` from the output directory.
2. Fix **only** places where the text conflicts with known context —
   misheard names, wrong technical terms, garbled proper nouns.
3. Do **not** rewrite style, fillers, phrasing, or anything that does not
   conflict with known facts. Keep the speaker's wording.
4. Write the corrected text back into `transcript.txt` (in place).
5. If the user asked for corrected subtitles, mirror the same fixes into
   `subtitles.srt` and `subtitles.vtt` (same timestamps, corrected words).
6. Leave `transcript.json` untouched — it is the raw archive and the source
   of truth.

**Never re-run cue on an already-corrected file.** cue regenerates
`transcript.txt` and subtitles from `transcript.json`, which would silently
discard your corrections. If a re-transcription is needed, preserve the
corrected text first (e.g., copy it aside).

### Worked example

A file `Talks/acme-dev-conf-2025/intro-to-observability/01-opening.mp4` is
transcribed, and the transcript contains:

> "I'm Dr. Ada River and I'm going to walk you through..."

The conference is "Acme Dev Conf", the talk is "Intro to Observability",
and the speaker's name is Dr. Ada **Rivas** (verifiable from the talk
page / schedule). Correct the transcript to read "Dr. Ada Rivas" and note
the fix when you report back. Do not "fix" anything else unless it also
conflicts with known context.

## 4. Batch / course mode

For a folder of related media (a course, a lecture series):

1. Create one shared context file next to the folder (or files) that
   captures what all episodes share — speaker name, platform, series,
   recurring terms:

   ```markdown
   # Talk / series context
   Platform: Acme Dev Conf
   Talk: Intro to Observability
   Speaker: Dr. Ada Rivas
   Terms: OpenTelemetry, traces, spans, metrics, distributed tracing
   ```

   See `references/context-file.md` in this skill for the full template.
2. Run cue once per episode (loop over the files), then apply the shared
   context corrections to each `transcript.txt`.
3. Optionally, if the runs produced `analysis.json` per episode, summarize
   them into a course index (titles, topics, chapters) for the user.

## 5. Privacy

Media, audio, the raw transcript, and subtitles stay local. Only cleaned
text is ever sent to a configured LLM gateway (for S1 normalization and
analysis). Never upload media files anywhere.

## 6. Troubleshooting

| Symptom | Fix |
|---|---|
| `cue doctor` reports FFmpeg/FFprobe missing | Install FFmpeg: `brew install ffmpeg` (macOS) / `apt install ffmpeg` (Linux) |
| `cue doctor` reports Python missing | Install Python 3.10+, then `cue doctor --fix` |
| Transcription fails on a short/quiet clip | Expected: whisper may return an empty transcript for music/silence/no-speech |
| No `transcript.clean.txt` / `summary.md` / `description.md` | Optional integrations (Ollama S1, LLM gateway) are not configured — local transcription still works |
| S1 cleanup needed | `cue models install s1` (pulls ~460 MB into Ollama) |
| Subtitle timing looks off | Re-run with a larger `max_duration_ms` in `~/.config/cue/cue.toml` `[subtitles]` |