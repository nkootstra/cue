//! Deterministic transcript corrections.
//!
//! A corrections manifest maps misheard phrases to their correct spelling
//! ("open telemetry" -> "OpenTelemetry", "John Dough" -> "John Doe"). The
//! engine parses the manifest and applies it to text mechanically, so the
//! fragile "an LLM must remember to edit files" step is eliminated: cue
//! rewrites transcripts and subtitles deterministically, and the agent only
//! has to *identify* the corrections.

use crate::CueError;

/// One correction rule: find `old`, replace with `new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    pub old: String,
    pub new: String,
}

/// Parse a corrections manifest.
///
/// Format: one rule per line, `old -> new`. `#` comments and blank lines
/// are ignored. A rule's `new` may be empty (delete the phrase) by writing
/// `old ->`; the phrase to find (`old`) must not be empty.
pub fn parse_manifest(text: &str) -> Result<Vec<Correction>, CueError> {
    let mut rules = Vec::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Split on the first ` ->` marker. `old -> new`, `old ->new`, and a
        // bare `old ->` (empty replacement, a deletion rule) all parse.
        let Some((old, new)) = line.split_once(" ->") else {
            return Err(CueError::general(format!(
                "corrections manifest line {} is not 'old -> new': {line:?}",
                index + 1
            ))
            .remedy(
                "write each rule as `phrase to find -> replacement`, or use `#` for comments",
            ));
        };
        let old = old.trim();
        rules.push(Correction {
            old: old.to_string(),
            new: new.trim().to_string(),
        });
    }
    Ok(rules)
}

/// Apply the correction rules to `text` in order, reporting how many
/// replacements fired.
///
/// Matching is case-insensitive and whole-phrase: a rule matches only where
/// the characters immediately before and after the phrase are not
/// alphanumeric, so "dough" never matches inside "doughnut". Case folding is
/// ASCII-only (byte offsets must stay stable), so phrases with non-ASCII
/// letters must be written in the same case as the transcript.
pub fn apply_counted(text: &str, rules: &[Correction]) -> (String, usize) {
    let (out, counts) = apply_with_counts(text, rules);
    (out, counts.into_iter().sum())
}

/// Apply correction rules in order and report the occurrence count for each
/// rule. The returned count vector always has the same length as `rules`.
pub fn apply_with_counts(text: &str, rules: &[Correction]) -> (String, Vec<usize>) {
    let mut out = text.to_string();
    let mut counts = Vec::with_capacity(rules.len());
    for rule in rules {
        let (replaced, count) = replace_phrase(&out, &rule.old, &rule.new);
        out = replaced;
        counts.push(count);
    }
    (out, counts)
}

/// Apply the correction rules to `text` in order.
pub fn apply(text: &str, rules: &[Correction]) -> String {
    apply_counted(text, rules).0
}

/// Count which rules would change the text at all (before application).
///
/// Matching uses the same whole-phrase boundary rule as [`apply`], so a rule
/// like `dough -> paste` does not count against "doughnut".
/// Useful for dry-run previews: report the rules that would change output.
pub fn matched_rules(text: &str, rules: &[Correction]) -> Vec<usize> {
    rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| replace_phrase(text, &rule.old, &rule.new).1 > 0)
        .map(|(index, _)| index)
        .collect()
}

fn replace_phrase(text: &str, old: &str, new: &str) -> (String, usize) {
    if old.is_empty() {
        return (text.to_string(), 0);
    }
    let lower_old = old.to_ascii_lowercase();
    let lower_text = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut search_from = 0usize;
    let mut cursor = 0usize;
    let mut count = 0usize;

    while let Some(start) = lower_text[search_from..].find(&lower_old) {
        let start = search_from + start;
        let end = start + old.len();
        // Whole-phrase boundary: the character before and after the phrase
        // must not be alphanumeric (char-aware, so unicode letters count).
        let start_ok = !text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric());
        let end_ok = !text[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric());
        if start_ok && end_ok {
            out.push_str(&text[cursor..start]);
            out.push_str(new);
            cursor = end;
            count += 1;
        }
        search_from = end;
    }
    out.push_str(&text[cursor..]);
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<Correction> {
        parse_manifest(
            "# test manifest\n\nopen telemetry -> OpenTelemetry\nJohn Dough -> John Doe\n",
        )
        .unwrap()
    }

    #[test]
    fn parses_rules_and_ignores_comments_and_blanks() {
        let rules = rules();
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules[0],
            Correction {
                old: "open telemetry".into(),
                new: "OpenTelemetry".into()
            }
        );
    }

    #[test]
    fn malformed_line_reports_line_number() {
        let err = parse_manifest("ok -> fine\nbad line here\n").unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("line 2"), "{rendered}");
        assert!(rendered.contains("old -> new"), "{rendered}");
    }

    #[test]
    fn missing_separator_is_an_error() {
        let err = parse_manifest("this is not a rule\n").unwrap_err();
        assert!(err.to_string().contains("old -> new"), "{err}");
    }

    #[test]
    fn applies_case_insensitively() {
        let corrected = apply(
            "see open telemetry and Open Telemetry and OPEN TELEMETRY.",
            &rules(),
        );
        assert_eq!(
            corrected,
            "see OpenTelemetry and OpenTelemetry and OpenTelemetry."
        );
    }

    #[test]
    fn whole_phrase_boundaries_protect_longer_words() {
        let corrected = apply(
            "the dough is in the doughnut",
            &[Correction {
                old: "dough".into(),
                new: "paste".into(),
            }],
        );
        assert_eq!(corrected, "the paste is in the doughnut");
    }

    #[test]
    fn multi_word_and_punctuation_bounded() {
        // Case-insensitive match; the replacement is used verbatim (John Doe
        // keeps its capital) and surrounding punctuation is preserved.
        let corrected = apply("(john dough) John Dough!", &rules());
        assert_eq!(corrected, "(John Doe) John Doe!");
    }

    #[test]
    fn empty_new_deletes_the_phrase() {
        let corrected = apply(
            "remove word now",
            &[Correction {
                old: "word".into(),
                new: String::new(),
            }],
        );
        assert_eq!(corrected, "remove  now");
    }

    #[test]
    fn applying_twice_is_a_noop() {
        let once = apply("open telemetry now", &rules());
        let twice = apply(&once, &rules());
        assert_eq!(once, twice);
        assert_eq!(once, "OpenTelemetry now");
    }

    #[test]
    fn apply_counted_reports_occurrences() {
        let (out, count) = apply_counted("open telemetry and open telemetry.", &rules());
        assert_eq!(out, "OpenTelemetry and OpenTelemetry.");
        assert_eq!(count, 2);
    }

    #[test]
    fn apply_with_counts_reports_each_ordered_rule() {
        let rules = [
            Correction {
                old: "foo".into(),
                new: "X".into(),
            },
            Correction {
                old: "X bar".into(),
                new: "Z".into(),
            },
        ];
        let (out, counts) = apply_with_counts("foo bar and foo", &rules);
        assert_eq!(out, "Z and X");
        assert_eq!(counts, vec![2, 1]);
    }

    #[test]
    fn rules_apply_in_file_order() {
        let corrected = apply(
            "foo bar",
            &[
                Correction {
                    old: "foo".into(),
                    new: "X".into(),
                },
                Correction {
                    old: "X bar".into(),
                    new: "Z".into(),
                },
            ],
        );
        assert_eq!(corrected, "Z");
    }

    #[test]
    fn matched_rules_reports_which_rules_would_fire() {
        let matched = matched_rules("traces with open telemetry.", &rules());
        assert_eq!(matched, vec![0]); // rule 1 fires, rule 2 (John Dough) does not
    }

    #[test]
    fn matched_rules_respects_whole_phrase_boundaries() {
        let rule = [Correction {
            old: "dough".into(),
            new: "paste".into(),
        }];
        // "dough" inside "doughnut" is not a whole-phrase match.
        assert!(matched_rules("the doughnut", &rule).is_empty());
        assert!(matched_rules("the dough", &rule) == vec![0]);
    }

    #[test]
    fn parses_deletion_rule_with_empty_replacement() {
        let rules = parse_manifest("remove this ->\nkeep -> that\n").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].old, "remove this");
        assert_eq!(rules[0].new, "");
        assert_eq!(rules[1].new, "that");
    }

    #[test]
    fn unicode_survives_replacement() {
        let corrected = apply("héllo open telemetry ünïcode", &rules());
        assert!(corrected.contains("OpenTelemetry"), "{corrected}");
        assert!(corrected.contains("héllo"), "{corrected}");
        assert!(corrected.contains("ünïcode"), "{corrected}");
    }
}
