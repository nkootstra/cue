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

**When you find a mishearing, fix it immediately, in place — do not defer
and do not ask permission.** Corrections are conservative by design: only
tokens that conflict with known context are changed, so applying them is
safe to do without a confirmation round-trip.

1. Read `transcript.txt` from the output directory.
2. Fix **only** places where the text conflicts with known context —
   misheard names, wrong technical terms, garbled proper nouns.
3. Do **not** rewrite style, fillers, phrasing, or anything that does not
   conflict with known facts. Keep the speaker's wording.
4. Write the corrected text back into `transcript.txt` (in place).
5. Mirror the same fixes into `subtitles.srt` and `subtitles.vtt` when they
   exist (same timestamps, corrected words). Do not skip subtitles; a name
   that appears there is a name that must be corrected there too.
6. Leave `transcript.json` untouched — it is the raw archive and the source
   of truth.

### Verify corrections

After applying corrections, confirm they actually landed:

1. Grep the corrected spelling in `transcript.txt` (e.g. the fixed name or
   term) and confirm it is present.
2. If a misheard variant remains anywhere, fix it and re-check.
3. Report what you changed (old -> new), so the user can review.

**Never re-run cue on an already-corrected file.** cue regenerates
`transcript.txt` and subtitles from `transcript.json`, which would silently
discard your corrections. If a re-transcription is needed, preserve the
corrected text first (e.g., copy it aside).

### Identity rule

Only correct a name when context gives you **high confidence**. If you
cannot verify the correct spelling (no reliable source, contradictory
cues), ask the user instead of guessing — a wrong correction is worse than
leaving the mishearing.

### Worked example

A file `Talks/acme-dev-conf-2025/intro-to-observability/01-opening.mp4` is
transcribed, and the transcript contains:

> "I'm John Dough and I'm going to walk you through..."

The talk page lists the speaker as John **Doe** (and the product as
**OpenTelemetry**). Correct the transcript to read "John Doe" and
"OpenTelemetry" (fixing the misheard surname and the garbled product name),
and note the fixes when you report back. Do not "fix" anything else unless
it also conflicts with known context.

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
3. **Loop.** For each media file: run cue, then correct its `transcript.txt`
   (and subtitles) against the shared context, verifying each correction as
   described in section 3. This includes existing `*.cue/` outputs found in
   step 1 — transcribing a new file is not a reason to leave older
   transcripts with known mishearings.
4. Optionally, if the runs produced `analysis.json` per episode, summarize
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