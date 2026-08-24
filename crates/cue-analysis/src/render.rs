//! Markdown renderers deriving presentation from a structured [`Analysis`].

use std::fmt::Write as _;

use cue_core::Analysis;

/// Render `summary.md`.
pub fn render_summary(analysis: &Analysis) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", analysis.title);
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", analysis.summary.trim());
    if !analysis.key_points.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "## Key points");
        for point in &analysis.key_points {
            let _ = writeln!(out, "- {}", point.trim());
        }
    }
    out
}

/// Render `description.md`: title, summary, keywords, chapters, points.
pub fn render_description(analysis: &Analysis) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", analysis.title);
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", analysis.summary.trim());

    if !analysis.keywords.is_empty() {
        let _ = writeln!(out);
        let tags: Vec<String> = analysis
            .keywords
            .iter()
            .map(|k| format!("#{}", k.trim().replace(' ', "")))
            .collect();
        let _ = writeln!(out, "{}", tags.join(" "));
    }

    if !analysis.topics.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "## Chapters");
        let _ = writeln!(out);
        for (title, start_ms) in analysis.chapters() {
            let seconds = start_ms / 1000;
            let _ = writeln!(out, "- {:02}:{:02} {}", seconds / 60, seconds % 60, title);
        }
    }

    if !analysis.key_points.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "## Key points");
        for point in &analysis.key_points {
            let _ = writeln!(out, "- {}", point.trim());
        }
    }
    out
}
