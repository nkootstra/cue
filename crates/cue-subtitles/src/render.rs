//! Subtitle file rendering: SubRip (SRT) and WebVTT.
//!
//! Renderers are pure functions from cues to text; they never re-flow or
//! second-guess segmentation.

use std::fmt::Write as _;

use crate::segment::Cue;

/// Render cues as SubRip (.srt).
///
/// ```text
/// 1
/// 00:00:01,000 --> 00:00:03,500
/// hello world.
/// ```
pub fn render_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (index, cue) in cues.iter().enumerate() {
        let _ = writeln!(out, "{}", index + 1);
        let _ = writeln!(
            out,
            "{} --> {}",
            format_srt_time(cue.start_ms),
            format_srt_time(cue.end_ms)
        );
        out.push_str(&cue.text);
        out.push_str("\n\n");
    }
    out
}

/// Render cues as WebVTT (.vtt), including the required header.
///
/// ```text
/// WEBVTT
///
/// 00:00:01.000 --> 00:00:03.500
/// hello world.
/// ```
pub fn render_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues {
        let _ = writeln!(
            out,
            "{} --> {}",
            format_vtt_time(cue.start_ms),
            format_vtt_time(cue.end_ms)
        );
        out.push_str(&cue.text);
        out.push_str("\n\n");
    }
    out
}

/// SRT timestamp: hours always present, millisecond separator is a comma.
fn format_srt_time(ms: u64) -> String {
    let (h, m, s, millis) = breakdown(ms);
    format!("{h:02}:{m:02}:{s:02},{millis:03}")
}

/// VTT timestamp: hours only when needed, millisecond separator is a dot.
fn format_vtt_time(ms: u64) -> String {
    let (h, m, s, millis) = breakdown(ms);
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}.{millis:03}")
    } else {
        format!("{m:02}:{s:02}.{millis:03}")
    }
}

fn breakdown(ms: u64) -> (u64, u64, u64, u64) {
    let millis = ms % 1000;
    let total_seconds = ms / 1000;
    let s = total_seconds % 60;
    let m = (total_seconds / 60) % 60;
    let h = total_seconds / 3600;
    (h, m, s, millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cues() -> Vec<Cue> {
        vec![
            Cue {
                start_ms: 1_000,
                end_ms: 3_500,
                text: "hello world.".into(),
            },
            Cue {
                start_ms: 3_600,
                end_ms: 65_250,
                text: "second line\ntext".into(),
            },
        ]
    }

    #[test]
    fn srt_indexes_from_one_and_uses_comma_millis() {
        let srt = render_srt(&sample_cues());
        insta::assert_yaml_snapshot!(srt);
    }

    #[test]
    fn vtt_has_header_and_dot_millis() {
        let vtt = render_vtt(&sample_cues());
        assert!(vtt.starts_with("WEBVTT\n\n"), "{vtt}");
        assert!(!vtt.contains(','), "vtt uses dots for milliseconds");
        insta::assert_yaml_snapshot!(vtt);
    }

    #[test]
    fn srt_hours_always_two_digits_vtt_omits_zero_hours() {
        let cue = Cue {
            start_ms: 3_600_000, // exactly one hour
            end_ms: 3_601_000,
            text: "late.".into(),
        };
        let srt = render_srt(std::slice::from_ref(&cue));
        assert!(srt.contains("01:00:00,000 --> 01:00:01,000"), "{srt}");

        let vtt = render_vtt(std::slice::from_ref(&cue));
        assert!(vtt.contains("01:00:00.000 --> 01:00:01.000"), "{vtt}");
    }

    #[test]
    fn empty_input_renders_minimal_files() {
        assert_eq!(render_srt(&[]), "");
        assert_eq!(render_vtt(&[]), "WEBVTT\n\n");
    }

    #[test]
    fn multi_line_cue_text_passes_through_verbatim() {
        let cue = Cue {
            start_ms: 0,
            end_ms: 1_000,
            text: "line one\nline two".into(),
        };
        let srt = render_srt(std::slice::from_ref(&cue));
        assert!(srt.contains("line one\nline two\n"), "{srt}");
    }
}
