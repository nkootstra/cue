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

Bounded parallel batches require cue 0.10.0 or newer. Upgrade before using
`--jobs`; do not emulate it with shell background jobs because that bypasses
cue's cache coordination, file-attributed progress, and deterministic failure
accounting.

The scoped lexicon workflow requires `cue review` and `cue lexicon` to appear
in `cue --help`. Upgrade cue before using review or promotion when either
command is absent; do not emulate promotion by silently copying rules.

Subtitle validation requires `cue subtitles` to appear in `cue --help`.
Upgrade cue before claiming subtitle-policy evidence when the command is
absent; visually inspecting SRT/VTT is useful but is not an equivalent check.

Attested output verification requires `cue verify` to appear in `cue --help`.
Upgrade cue before claiming that generated files still match their completed
run receipt when that command is absent.

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
cue --recursive <directory> --jobs 2
```

cue runs the whole local pipeline: inspect -> extract audio -> transcribe ->
normalize (S1, if installed) -> analyze (if a gateway is configured) ->
write outputs. Outputs land in `<file>.cue/` next to the source (or `--output`):

| File | Role |
|---|---|
| `transcript.json` | **Raw canonical transcript.** Timed words, the source of truth. Never edit this. |
| `transcript.txt` | Plain text derived from the canonical transcript. Inspect this to identify corrections. |
| `normalized.json` / `transcript.clean.txt` | S1-cleaned prose (present when Ollama has the S1 model). |
| `subtitles.srt` / `subtitles.vtt` | Subtitles derived from the canonical transcript. |
| `analysis.json` / `summary.md` / `description.md` | LLM analysis (present when a gateway is configured). |
| `corrections.applied.json` | Receipt written after correction, showing which rules were applied to `transcript.txt`, optional `transcript.clean.txt`, and subtitles. |
| `cue.run.json` | Completion receipt binding the source, effective configuration, providers, correction manifests, and final artifact hashes. |

After processing, verify the completed output before reporting success:

```bash
cue verify <file>.cue
cue verify <file>.cue --json  # when another agent or script consumes the result
```

Exit status 0 means the source, corrections, and artifacts still match the
receipt. Inspect status-1 output: it can mean drift, a missing file, or an
unreadable receipt. Do not edit `cue.run.json` by hand. Treat the receipt as an
integrity record, not a signature or proof of authorship, and only trust
receipts from a trusted source. Its remote-data field describes transfers made
by the current run only; cached artifacts do not establish earlier provider
lineage.

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

Results keep the positional path-group order supplied on the command line,
with files from each directory sorted lexically. Hidden media files are
included. cue resolves canonical paths and processes the same media only once,
even when it is reached through multiple inputs or a file symlink. Directory
symlinks are never traversed; an explicitly supplied directory symlink is
rejected, so pass its resolved target directory instead.

A directory or multiple positional paths is a batch. Without `--output`, each
file writes `<stem>.cue/` beside its source. With `--output <root>`, a batch
writes `<root>/<stem>.cue/`; a single explicit file still uses `--output` as
the output directory itself. cue processes batch files sequentially by default.
Use `--jobs <N>` for bounded parallel processing, starting with `--jobs 2`
because transcription and local model stages can consume substantial memory.
cue keeps progress attributable to each file, continues after individual media
failures, reports failures in input order, and exits with status 1 after its
summary if anything failed. If two inputs would have the same destination,
resolve the name collision before retrying.

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
Your job is to *identify* corrections from context; cue applies them to every
`transcript.txt`, optional `transcript.clean.txt`, and subtitle artifact
mechanically. Keeping the manifest lets cached reruns
reapply the same decisions without changing the raw transcript.

1. Run a focused review, then inspect the relevant surrounding text:

   ```bash
   cue review <file>.cue
   cue review <file>.cue --json
   ```

   The report is read-only. Low confidence, possible fallback timing,
   unmatched rules, scoped conflicts, and speaker ambiguity are candidates to
   investigate—not facts and not permission to guess.
2. Identify **only** places where the text conflicts with known context —
   misheard names, wrong technical terms, garbled proper nouns. Do **not**
   rewrite style, fillers, phrasing, or anything that does not conflict with
   known facts.
3. Write a `corrections.md` manifest at the narrowest appropriate scope, one rule
   per line in `phrase to find -> replacement` form:

   ```text
   # corrections.md — applied by cue during rendering
   open telemetry -> OpenTelemetry
   John Dough -> John Doe
   ```

   See `references/corrections-file.md` in this skill for the full format.
4. Apply the manifest. For media that should be processed or safely rerun,
   pass the manifest during the main invocation:

   ```bash
   cue --corrections /path/to/corrections.md <file.mp4>
   ```

   cue reuses cached transcription, normalization, and analysis, then rebuilds
   the derived files and applies the current manifest. Without an explicit
   flag, cue composes scoped `corrections.md` files from the current working
   directory down through nearer ancestor folders and the output. A nearer
   mapping wins a same-phrase conflict. Run from the intended project/course
   root so that boundary is explicit.

   For an existing output that should not run through media processing, preview
   and rebuild it directly:

   ```bash
   cue correct <file>.cue --dry-run
   cue correct <file>.cue
   ```

   If the manifest lives elsewhere, pass it explicitly: `cue correct
   <file>.cue --corrections /path/to/corrections.md`.

5. `cue correct` reconstructs `transcript.txt`, cleaned text, and SRT/VTT
   files from canonical JSON before applying rules. It overwrites manual edits
   in those derived files. It **never touches `transcript.json`,
   `normalized.json`, or analysis outputs**.

### Verify corrections

After applying, confirm the fixes actually landed:

1. When the correction was applied by processing the source, run `cue verify
   <file>.cue`. A direct `cue correct` intentionally removes `cue.run.json`
   because the previous run no longer attests to the changed files; rerun the
   source with the manifest to create a fresh receipt when attestation is
   required.
2. Grep the corrected spelling in `transcript.txt` (and `subtitles.srt`) and
   confirm it is present.
3. Confirm `corrections.applied.json` exists. Treat the manifest, not this
   generated receipt, as the durable source of correction rules.
4. Check the rebuilt subtitle cues:

   ```bash
   cue subtitles check <file>.cue
   cue subtitles check <file>.cue --json
   ```

   The command is read-only and links each duration, line-capacity,
   reading-speed, or timing-repair finding to its canonical half-open word
   range. Exit status 0 means no findings; status 1 means findings or an
   operational error, so inspect the output rather than treating every status
   1 as a failed invocation. Use JSON when another agent or script consumes
   the diagnostics.
5. If a misheard variant remains, add it to the manifest and re-run
   `cue correct`.
6. Report what you changed (old -> new) and any remaining subtitle findings,
   so the user can review.

### Promote verified corrections

Promotion is always a separate, deliberate action. Only promote when the user
asked to reuse the correction or explicitly approved the destination scope:

```bash
cue lexicon promote <file>.cue \
  --rule "open telemetry" \
  --to /path/to/project-or-course \
  --json
```

The command reads `corrections.applied.json`, verifies that its manifest and
canonical-source hashes still match, and accepts only a rule that made at least
one replacement. If files changed, rerun `cue correct` before promoting. The
target directory must already exist. Promotion is idempotent and refuses to
overwrite a conflicting mapping. Never silently promote every correction,
choose a broader scope on the user's behalf, or edit the generated receipt.
Use `--json` when a workflow needs to retain machine-readable evidence bound
to the source receipt and resulting target lexicon.

Keep the manifest available whenever you rerun the source. cue regenerates
derived files from canonical JSON and reapplies the current manifest. Changing
the manifest replaces earlier corrections instead of compounding them;
removing it restores uncorrected derived text and removes the receipt.

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

then run `cue --corrections corrections.md 01-opening.mp4`. If the media was
already processed, the expensive stages come from cache. Use `cue correct
01-opening.cue --dry-run` and `cue correct 01-opening.cue` when you only need
to rebuild the existing output. Do not add any other corrections unless they
also conflict with known context.

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
3. **Build the shared corrections manifest deliberately.** Start narrow,
   apply and verify a correction, then promote it to the course directory when
   the user approves that shared scope. You may write a shared manifest
   directly when the supplied context already proves it applies across the
   course:

   ```text
   # corrections.md — applied to every episode
   John Dough -> John Doe
   open telemetry -> OpenTelemetry
   ```

4. **Transcribe pending inputs with the shared lexicon.** Run from the course
   or project root so hierarchical discovery has an explicit boundary. Use one cue
   invocation for multiple explicit files, or cue's directory mode:

   ```bash
   cue <first.mp4> <second.mp4>
   cue --recursive <course-directory> --jobs 2
   ```

   Directory mode requires cue 0.5.0 or newer; `--jobs` requires cue 0.10.0 or
   newer. cue discovers media and creates a sibling `<stem>.cue/` per source by
   default. Omit `--jobs` when memory is constrained or only one file should
   run at a time.
   Pass `--corrections /absolute/path/to/corrections.md` when deliberately
   bypassing hierarchical discovery. A rerun safely reuses the pipeline cache
   and reapplies the effective lexicon.

   Use `cue correct` against existing output directories when no media
   processing is needed:

   ```bash
   cue correct <course-directory/episode.cue>
   cue correct <course-directory/module/episode.cue>
   ```

   Run those commands from the course root. If the outputs are outside that
   root, pass the shared manifest explicitly instead.

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
| Subtitle timing or readability looks off | Run `cue subtitles check <file>.cue`; inspect the reported canonical word ranges before changing `[subtitles]` limits |
