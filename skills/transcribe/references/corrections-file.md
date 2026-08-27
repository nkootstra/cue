# Corrections file template

A corrections manifest is a plain-text file (`corrections.md`) that maps
misheard phrases to their correct spelling. It is applied deterministically
by normal cue processing and `cue correct`, so the agent's job is to
*identify* the corrections, not to edit transcripts by hand.

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
  (e.g. `a -> b` followed by `b -> c`): both rules run during one render,
  which can obscure the intended correction.
- `replacement` may be empty to delete a phrase (`remove this ->`); the
  phrase to find must not be empty.

## Worked example

```text
# corrections.md — applied by cue during rendering
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
- **Apply during normal processing or a cached rerun:**

  ```bash
  cue --corrections corrections.md <file.mp4>
  ```

- **Preview or rebuild an existing output:**

  ```bash
  cue correct <file>.cue --corrections corrections.md --dry-run
  cue correct <file>.cue --corrections corrections.md
  ```

- **Batch folders:** keep one shared `corrections.md` and pass it to the main
  batch command with `--corrections`. This also works for recursive outputs
  with different parents.

Without an explicit flag, cue discovers `corrections.md` in the output
directory and then its parent. An explicit manifest always takes precedence.
The manifest remains authoritative: changing it replaces previous corrections
from canonical data, while removing it restores uncorrected derived files on
the next normal render.

## What correction rendering touches

- Reconstructs and rewrites: `transcript.txt`, cleaned text when
  `normalized.json` exists, and configured or existing SRT/VTT subtitles.
- Writes `corrections.applied.json`, a versioned receipt with canonical source
  hashes and per-rule application counts. Do not edit or reuse it as a
  manifest.
- Never modifies: `transcript.json`, `normalized.json`, or analysis outputs.
- Overwrites manual edits in derived text and subtitle files because every
  correction render starts from canonical JSON.
