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
cue --version
cue doctor
```

Directory input requires cue 0.5.0 or newer. Before using directory syntax,
check `cue --version`; upgrade cue with the same installation method if it is
older. Do not substitute a hand-written directory loop for an outdated cue
without telling the user why the upgrade could not be completed.

If the required tools are fine but the Python worker is missing, provision it:

```bash
cue doctor --fix
```

`cue doctor` reports optional integrations (Ollama, S1, an LLM gateway)
separately — those are not required for transcription.

## 2. Transcribe files and directories

```bash
cue <file.mp4> [--language en] [--output <dir>]
cue <first.mp4> <second.wav>
cue <directory>
cue --recursive <directory>
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
cue transcribe <path> [<path>...]
```

Both modes accept one or more explicit media files and directories. Directory
inputs scan only their top level unless `--recursive` is present. Directory
discovery recognizes these extensions case-insensitively: `aac`, `aif`,
`aiff`, `flac`, `m4a`, `mp3`, `ogg`, `opus`, `wav`, `avi`, `m2ts`, `m4v`,
`mkv`, `mov`, `mp4`, `mpeg`, `mpg`, `mts`, `ts`, and `webm`. Explicit files
may use another extension because FFprobe still decides whether they are
media.

A directory or multiple positional paths is a batch. Without `--output`, each
file writes `<stem>.cue/` beside its source. With `--output <root>`, a batch
writes `<root>/<stem>.cue/`; a single explicit file still uses `--output` as
the output directory itself. cue processes batch files sequentially, continues
after individual media failures, and exits with status 1 after its summary if
anything failed. If two inputs would have the same destination, resolve the
name collision before retrying.

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

**Corrections are applied deterministically, not by editing files by hand.**
Your job is to *identify* the corrections from context; `cue correct` applies
them to every text artifact (transcript, subtitles) mechanically. This
guarantees the fix lands everywhere and never corrupts the raw transcript.

1. Read `transcript.txt` from the output directory.
2. Identify **only** places where the text conflicts with known context —
   misheard names, wrong technical terms, garbled proper nouns. Do **not**
   rewrite style, fillers, phrasing, or anything that does not conflict with
   known facts.
3. Write a `corrections.md` manifest next to the output directory, one rule
   per line in `phrase to find -> replacement` form:

   ```text
   # corrections.md — applied by: cue correct <file>.cue
   open telemetry -> OpenTelemetry
   John Dough -> John Doe
   ```

   See `references/corrections-file.md` in this skill for the full format.
4. Preview and apply. The manifest is auto-discovered from the output
   directory (or its parent), so no `--corrections` flag is needed when the
   manifest sits next to the outputs:

   ```bash
   cue correct <file>.cue --dry-run
   cue correct <file>.cue
   ```

   If the manifest lives elsewhere, pass it explicitly: `cue correct
   <file>.cue --corrections /path/to/corrections.md`.

5. `cue correct` rewrites `transcript.txt`, `transcript.clean.txt`, and
   `subtitles.srt`/`.vtt` in place, reports per-file replacement counts, and
   **never touches `transcript.json`** (the raw archive) or analysis outputs.

### Verify corrections

After applying, confirm the fixes actually landed:

1. Grep the corrected spelling in `transcript.txt` (and `subtitles.srt`) and
   confirm it is present.
2. If a misheard variant remains, add it to the manifest and re-run
   `cue correct`.
3. Report what you changed (old -> new), so the user can review.

**Never re-run the processing pipeline for media whose `.cue` output has
already been corrected.** A rerun regenerates `transcript.txt` and subtitles
from `transcript.json`, silently discarding corrections. Do not pass `.cue`
output directories as media inputs, and do not rerun an encompassing source
directory after correcting any of its outputs. If re-transcription is needed,
preserve the corrected text first (for example, copy it aside).

### Identity rule

Only include a correction when context gives you **high confidence**. If you
cannot verify the correct spelling (no reliable source, contradictory cues),
ask the user instead of guessing — a wrong correction is worse than leaving
the mishearing.

Never present an unverified speaker or platform identity as fact. If you name
who is speaking in a report or a context file, either verify it from a
reliable source or explicitly mark it as unverified (e.g. "speaker appears to
be X — unverified"), and never base a correction on an unverified guess.

### Worked example

A file `Talks/acme-dev-conf-2025/intro-to-observability/01-opening.mp4` is
transcribed, and the transcript contains:

> "I'm John Dough and I'm going to walk you through..."

The talk page lists the speaker as John **Doe** (and the product as
**OpenTelemetry**). Write a `corrections.md` next to the outputs:

```text
John Dough -> John Doe
open telemetry -> OpenTelemetry
```

then run `cue correct 01-opening.cue --dry-run` (auto-discovers the manifest)
and apply it. Do not add any other corrections unless they also conflict
with known context.

## 4. Batch / course mode

For a folder of related media (a course, a lecture series), follow this
order:

1. **Scan the folder first.** List the media files and look for any existing
   `*.cue/` output directories. If outputs already exist, read their
   `transcript.txt` files — they may contain mishearings that the shared
   context should correct, and they may already reveal the speaker and terms
   you need.
2. **Write the shared context file before transcribing anything.** Create
   `context.md` next to the folder (or files) capturing what all episodes
   share — speaker name, platform, series, recurring terms:

   ```markdown
   # Talk / series context

   ## Speaker
   - John Doe (main presenter)

   ## Platform
   - Acme Dev Conf 2025

   ## Material
   - Talk: Intro to Observability

   ## Terms
   - OpenTelemetry
   - traces
   - spans
   - metrics
   ```

   See `references/context-file.md` in this skill for the full template.
   This file persists your context so every episode is corrected
   consistently, not just the one you transcribed first.
3. **Write a shared corrections manifest.** After you have gathered enough
   context (step 1-2), write one `corrections.md` for the folder capturing
   the mishearings shared across episodes — speaker name, product terms:

   ```text
   # corrections.md — applied to every episode
   John Dough -> John Doe
   open telemetry -> OpenTelemetry
   ```

4. **Transcribe pending inputs together, then apply.** Use one cue invocation
   for multiple explicit files, or cue's directory mode for a new tree:

   ```bash
   cue <first.mp4> <second.mp4>
   cue --recursive <course-directory>
   ```

   Directory mode requires cue 0.5.0 or newer. cue discovers media, creates a
   sibling `<stem>.cue/` per source by default, and processes it sequentially.
   If the scan in step 1 found corrected outputs, do not rerun the encompassing
   directory; pass only the unprocessed source files explicitly.

   Then run `cue correct` against every new and existing output directory.
   For recursive inputs, outputs can have different parents, so always pass
   the shared manifest explicitly rather than relying on auto-discovery:

   ```bash
   cue correct <course-directory/episode.cue> --corrections /absolute/path/to/corrections.md
   cue correct <course-directory/module/episode.cue> --corrections /absolute/path/to/corrections.md
   ```

   For a flat batch whose outputs and manifest share one parent,
   auto-discovery remains sufficient: `cue correct <file>.cue`.

5. Optionally, if the runs produced `analysis.json` per episode, summarize
   them into a course index (titles, topics, chapters) for the user.

## 5. Privacy

Media, extracted audio, the raw transcript, and subtitles stay local. S1
normalization runs through the user's local Ollama instance. If analysis is
enabled, cue sends only the normalized transcript text to the configured LLM
gateway. A local Ollama analysis gateway keeps that text on the machine; a
remote gateway such as OpenRouter receives it. Never upload media files
anywhere.

## 6. Troubleshooting

| Symptom | Fix |
|---|---|
| `cue doctor` reports FFmpeg/FFprobe missing | Install FFmpeg: `brew install ffmpeg` (macOS) / `apt install ffmpeg` (Linux) |
| `cue doctor` reports Python missing | Install Python 3.10+, then `cue doctor --fix` |
| Transcription fails on a short/quiet clip | Expected: whisper may return an empty transcript for music/silence/no-speech |
| No `transcript.clean.txt` / `summary.md` / `description.md` | Optional integrations (Ollama S1, LLM gateway) are not configured — local transcription still works |
| S1 cleanup needed | `cue models install s1` (pulls ~460 MB into Ollama) |
| `cue models install s1` fails with a 400 from Ollama | A known Ollama API limitation with `FROM hf.co/...` Modelfiles; run `ollama create cue-s1-mini -f <path-to-Modelfile>` manually and retry |
| Subtitle timing looks off | Re-run with a larger `max_duration_ms` in `~/.config/cue/cue.toml` `[subtitles]` |
