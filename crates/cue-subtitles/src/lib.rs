//! Subtitle generation from the canonical transcript.

pub mod lines;
pub mod render;
pub mod segment;

pub use lines::break_lines;
pub use render::{render_srt, render_vtt};
pub use segment::{segment, Cue, SubtitlePolicy};

use cue_core::Transcript;

/// Build finished subtitle cues: segment the transcript, then apply line
/// breaking per the policy.
pub fn build_cues(transcript: &Transcript, policy: &SubtitlePolicy) -> Vec<Cue> {
    let mut cues = segment(transcript, policy);
    for cue in &mut cues {
        cue.text = break_lines(&cue.text, policy);
    }
    cues
}

#[cfg(test)]
mod tests {
    use super::segment::{segment, Cue, SubtitlePolicy};
    use cue_core::{Segment, Transcript, Word, TRANSCRIPT_SCHEMA_VERSION};    fn word(text: &str, start_ms: u64, end_ms: u64) -> Word {
        Word {
            text: text.into(),
            start_ms,
            end_ms,
            confidence: Some(0.9),
            speaker: None,
        }
    }

    fn transcript(words: Vec<Word>) -> Transcript {
        // One spoken segment spanning every word; segmentation tests care
        // about cue boundaries inside it.
        let word_start = 0;
        let word_end = words.len();
        let start_ms = words.first().map(|w| w.start_ms).unwrap_or(0);
        let end_ms = words.last().map(|w| w.end_ms).unwrap_or(0);
        Transcript {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            language: "en".into(),
            duration_ms: end_ms,
            words,
            segments: vec![Segment {
                start_ms,
                end_ms,
                text: String::new(),
                word_start,
                word_end,
            }],
        }
    }

    #[test]
    fn empty_transcript_yields_no_cues() {
        let t = transcript(vec![]);
        assert!(segment(&t, &SubtitlePolicy::default()).is_empty());
    }

    #[test]
    fn sentences_close_cues_at_punctuation() {
        let t = transcript(vec![
            word("hello", 0, 300),
            word("world.", 310, 600),
            word("second", 700, 900),
            word("sentence.", 910, 1_200),
        ]);
        let cues = segment(&t, &SubtitlePolicy::default());

        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "hello world.");
        assert_eq!(cues[0].start_ms, 0);
        assert_eq!(cues[0].end_ms, 600);
        assert_eq!(cues[1].text, "second sentence.");
    }

    #[test]
    fn pauses_split_unpunctuated_speech() {
        let t = transcript(vec![
            word("before", 0, 300),
            word("pause", 310, 500),
            word("after", 1_400, 1_600), // 900ms gap >= PAUSE_BREAK_MS
            word("pause", 1_610, 1_800),
        ]);
        let cues = segment(&t, &SubtitlePolicy::default());
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "before pause");
        assert_eq!(cues[1].text, "after pause");
    }

    #[test]
    fn long_runs_split_at_character_budget() {
        // A single sentence far wider than the default budget (2*42+1).
        let mut words = Vec::new();
        let mut ms = 0u64;
        for i in 0..40 {
            let text = if i == 39 { "endless.".to_string() } else { format!("word{i:02}") };
            words.push(word(&text, ms, ms + 100));
            ms += 150;
        }
        let t = transcript(words);

        let policy = SubtitlePolicy::default();
        let cues = segment(&t, &policy);

        assert!(cues.len() > 1, "expected multiple cues");
        for cue in &cues {
            assert!(
                cue.text.chars().count() <= policy.char_budget(),
                "cue exceeds budget: {:?} ({} chars)",
                cue.text,
                cue.text.chars().count()
            );
        }
        // Cues remain ordered and non-overlapping.
        for pair in cues.windows(2) {
            assert!(pair[0].end_ms <= pair[1].start_ms + 1);
        }
    }

    #[test]
    fn max_duration_splits_marathon_words() {
        let policy = SubtitlePolicy {
            max_duration_ms: 1_000,
            ..SubtitlePolicy::default()
        };
        // No punctuation, no pauses — only duration can break these.
        let t = transcript(vec![
            word("a", 0, 500),
            word("b", 500, 1_000),
            word("c", 1_000, 1_500),
            word("d", 1_500, 2_000),
        ]);
        let cues = segment(&t, &policy);
        assert!(
            cues.len() >= 2,
            "duration budget should force a split: {cues:?}"
        );
    }

    #[test]
    fn unicode_counts_characters_not_bytes() {
        let policy = SubtitlePolicy {
            max_lines: 1,
            max_chars_per_line: 5,
            max_duration_ms: 60_000,
            max_chars_per_second: None,
        };
        // Each emoji is one char but four bytes.
        let t = transcript(vec![
            word("😀😀😀", 0, 100),
            word("😀😀😀", 110, 200),
        ]);
        let cues = segment(&t, &policy);
        // Budget is 5 chars; each word is 3 chars so they cannot share a cue
        // (3 + 1 + 3 > 5).
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text.chars().count(), 3);
    }

    #[test]
    fn overlapping_word_times_are_clamped_monotonic() {
        let t = transcript(vec![
            word("one.", 0, 500),
            word("two.", 400, 900), // starts before previous ends
        ]);
        let cues = segment(&t, &SubtitlePolicy::default());
        assert_eq!(cues.len(), 2);
        assert!(
            cues[0].end_ms <= cues[1].start_ms,
            "cues must not overlap: {cues:?}"
        );
    }

    #[test]
    fn very_short_words_survive_and_keep_order() {
        let t = transcript(vec![
            word("a", 0, 20),
            word("i", 30, 40),
            word("o", 50, 70),
        ]);
        let cues = segment(&t, &SubtitlePolicy::default());
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "a i o");
        assert!(cues[0].end_ms >= cues[0].start_ms);
    }

    #[test]
    fn reading_speed_policy_is_represented() {
        let policy = SubtitlePolicy {
            max_chars_per_second: Some(17.0),
            ..SubtitlePolicy::default()
        };
        assert_eq!(policy.max_chars_per_second, Some(17.0));
        assert_eq!(policy.char_budget(), 85);
    }

    #[test]
    fn cues_carry_sorted_times_within_themselves() {
        let t = transcript(vec![
            word("fine.", 100, 400),
            word("later.", 800, 1_100),
        ]);
        let cues: Vec<Cue> = segment(&t, &SubtitlePolicy::default());
        for cue in &cues {
            assert!(cue.start_ms <= cue.end_ms, "{cue:?}");
        }
    }

    #[test]
    fn build_cues_applies_line_breaking() {
        use super::build_cues;

        // A cue long enough to wrap at 20 chars per line.
        let t = transcript(vec![
            word("the", 0, 100),
            word("quick", 110, 200),
            word("brown", 210, 300),
            word("fox", 310, 400),
            word("jumps.", 410, 500),
            word("more", 600, 700),
            word("words", 710, 800),
            word("here.", 810, 900),
        ]);
        let policy = SubtitlePolicy {
            max_lines: 2,
            max_chars_per_line: 20,
            ..SubtitlePolicy::default()
        };
        let cues = build_cues(&t, &policy);
        assert!(!cues.is_empty());
        for cue in &cues {
            for line in cue.text.lines() {
                assert!(line.chars().count() <= 20, "{:?}", cue.text);
            }
        }
    }
}
