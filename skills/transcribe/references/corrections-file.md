# Corrections file template

A corrections manifest is a plain-text file (`corrections.md`) that maps
misheard phrases to their correct spelling. It is applied deterministically
by `cue correct`, so the agent's job is to *identify* the corrections, not
to edit transcripts by hand.

## Format

One rule per line:

```text
phrase to find -> replacement
```

- Lines starting with `#` and blank lines are ignored.
- Matching is **case-insensitive** and **whole-phrase**: a rule matches only
  where the phrase is bounded by non-alphanumeric characters, so `dough`
  never matches inside `doughnut`. Multi-word phrases are fine.
- Rules apply **in order** — list specific phrases before generic ones.
  Avoid chains where one rule's replacement matches another rule's phrase
  (e.g. `a -> b` followed by `b -> c`): such chains change output again on a
  second pass, so a manifest is only a no-op on re-apply when its rules do
  not overlap.
- `replacement` may be empty to delete a phrase (`remove this ->`); the
  phrase to find must not be empty.

## Worked example

```text
# corrections.md — applied by: cue correct <file>.cue
John Dough -> John Doe
open telemetry -> OpenTelemetry
acme dev conf -> Acme Dev Conf
```

This is a fictional conference talk; real manifests name the actual
speaker, products, and terms from context.

## Tips

- **Names first.** Speaker and guest names are the most commonly misheard
  words; include them even when they seem obvious.
- **Product and technical terms** the speech engine garbles
  ("OpenTelemetry" -> "open telemetry", "cargo" -> "kargo").
- **Only include corrections you are confident about.** A wrong correction
  is worse than none — ask the user when unsure.
- **Preview before applying:**

  ```bash
  cue correct <file>.cue --corrections corrections.md --dry-run
  cue correct <file>.cue --corrections corrections.md
  ```

- **Batch folders:** put one `corrections.md` in the folder and apply it to
  every `<file>.cue` output, including ones transcribed earlier.

## What cue correct touches

- Rewrites: `transcript.txt`, `transcript.clean.txt`, `subtitles.srt`,
  `subtitles.vtt` (only the ones that exist).
- Never touches: `transcript.json` (the raw archive) or analysis outputs.