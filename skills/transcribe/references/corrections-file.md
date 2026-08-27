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

- **Batch folders:** place one shared `corrections.md` at the course or
  project scope and run cue from that directory, or pass it explicitly with
  `--corrections`.

Without an explicit flag, outputs beneath the current working directory
compose every `corrections.md` from that directory down through nearer folder
scopes and the output directory. Broad rules apply first; the nearest mapping
wins when scopes define the same phrase. Outputs outside the working directory
use only their output-local and direct-parent files. An explicit manifest
bypasses discovery. Manifests remain authoritative: changing them replaces
previous corrections from canonical data, while removing all applicable
manifests restores uncorrected derived files on the next normal render.

## Review and promotion

Run a focused, read-only review before inventing corrections:

```bash
cue review <file>.cue
cue review <file>.cue --json
```

Review candidates include low-confidence words, possible fallback timing,
unmatched rules, scoped mapping conflicts, and ambiguous speaker turns. They
are leads to verify against context, not permission to guess.

Promote a successfully applied rule only when the user has approved the
destination scope:

```bash
cue lexicon promote <file>.cue \
  --rule "open telemetry" \
  --to /path/to/course \
  --json
```

The target must be an existing directory. Promotion verifies that the receipt
still matches its manifest and canonical sources, requires at least one
recorded replacement, avoids duplicates, and refuses to replace a conflicting
rule. If the sources changed, rerun `cue correct` before promoting. Never copy
rules between scopes by assumption or silently promote every applied
correction.
`--json` emits a machine-readable attestation containing hashes of the source
receipt and resulting target lexicon.

## What correction rendering touches

- Reconstructs and rewrites: `transcript.txt`, cleaned text when
  `normalized.json` exists, and configured or existing SRT/VTT subtitles.
- Writes `corrections.applied.json`, a versioned receipt with every
  contributing manifest and hash, canonical source hashes, winning rule
  provenance, and per-rule application counts. Do not edit or reuse it as a
  manifest.
- Never modifies: `transcript.json`, `normalized.json`, or analysis outputs.
- Overwrites manual edits in derived text and subtitle files because every
  correction render starts from canonical JSON.
