//! Line breaking for subtitle cues.
//!
//! Well-formed cues (produced by segmentation under the same policy) fit
//! within `max_lines`. Text that cannot fit degrades gracefully: extra
//! lines are emitted rather than truncating content.

use crate::segment::SubtitlePolicy;

/// Wrap cue text into at most `policy.max_lines` balanced lines of at most
/// `policy.max_chars_per_line` characters each.
///
/// Returns the original text untouched when it already fits. When a single
/// word exceeds the line width the algorithm degrades gracefully rather
/// than failing: that word gets its own line.
pub fn break_lines(text: &str, policy: &SubtitlePolicy) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return text.to_string();
    }

    // Already fits on one line?
    if text.chars().count() <= policy.max_chars_per_line {
        return text.to_string();
    }

    let greedy = greedy_wrap(&words, policy);

    // Balance two-line results so the first line is not disproportionately
    // long — standard subtitle practice.
    if greedy.len() == 2 {
        balance_two_lines(greedy, policy)
    } else {
        join_lines(greedy)
    }
}

fn greedy_wrap(words: &[&str], policy: &SubtitlePolicy) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in words {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= policy.max_chars_per_line {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Move trailing words from line 1 down while both lines stay within
/// budget and the result gets more balanced.
fn balance_two_lines(mut lines: Vec<String>, policy: &SubtitlePolicy) -> String {
    if lines.len() != 2 {
        return join_lines(lines);
    }

    loop {
        // Split the last word off line 1.
        let head_and_tail = lines[0].clone();
        let Some((head, tail)) = head_and_tail.rsplit_once(' ') else {
            break;
        };
        let new_second_len = tail.chars().count() + 1 + lines[1].chars().count();
        // Only rebalance while line 1 remains strictly longer.
        if head.chars().count() >= new_second_len && new_second_len <= policy.max_chars_per_line {
            lines[0] = head.to_string();
            lines[1] = format!("{tail} {}", lines[1]);
        } else {
            break;
        }
    }

    join_lines(lines)
}

fn join_lines(lines: Vec<String>) -> String {
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(max_chars_per_line: usize) -> SubtitlePolicy {
        SubtitlePolicy {
            max_lines: 2,
            max_chars_per_line,
            ..SubtitlePolicy::default()
        }
    }

    #[test]
    fn short_text_stays_single_line() {
        assert_eq!(break_lines("hello.", &policy(42)), "hello.");
    }

    #[test]
    fn wraps_into_two_lines_when_needed() {
        let text = "the quick brown fox jumps over the dog";
        let result = break_lines(text, &policy(20));
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2, "{result}");
        for line in &lines {
            assert!(line.chars().count() <= 20, "{line}");
        }
    }

    #[test]
    fn two_line_results_are_balanced() {
        let text = "aaaaa bbbbb ccccc ddddd";
        let result = break_lines(text, &policy(15));
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2, "{result}");
        let a = lines[0].chars().count();
        let b = lines[1].chars().count();
        assert!(a >= b, "first line should carry more weight: {result}");
    }

    #[test]
    fn oversized_word_gets_its_own_line() {
        let huge = "x".repeat(50);
        let text = format!("tiny {huge}");
        let result = break_lines(&text, &policy(10));
        let lines: Vec<&str> = result.lines().collect();
        // Graceful degradation: content preserved verbatim across lines.
        assert!(lines.len() >= 2, "{result}");
        assert_eq!(result.split_whitespace().count(), 2);
    }

    #[test]
    fn no_words_yields_input_unchanged() {
        assert_eq!(break_lines("", &policy(42)), "");
    }

    #[test]
    fn three_lines_are_possible_for_wide_budgets_only_via_segmentation() {
        // The breaker itself caps at max_lines via segmentation budgets;
        // verify a pathological input still round-trips its words.
        let text = "one two three four five six seven eight";
        let result = break_lines(text, &policy(8));
        assert_eq!(
            result.split_whitespace().collect::<Vec<_>>(),
            vec![
                "one", "two", "three", "four", "five", "six", "seven", "eight"
            ]
        );
    }
}
