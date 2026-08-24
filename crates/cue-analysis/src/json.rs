//! Lenient JSON extraction from LLM responses.
//!
//! Models occasionally wrap JSON in markdown fences or prose despite
//! instructions; extract the outermost balanced JSON object before parsing.

/// Extract and parse the first balanced JSON object in `raw`.
pub fn parse_lenient_json(raw: &str) -> cue_core::Result<serde_json::Value> {
    let text = strip_fences(raw);
    let bytes = text.as_bytes();
    let Some(start) = text.find('{') else {
        return Err(invalid(raw));
    };

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, byte) in bytes.iter().enumerate().skip(start) {
        match byte {
            _ if in_string => match byte {
                b'\\' => escaped = !escaped,
                b'"' if !escaped => in_string = false,
                _ => escaped = false,
            },
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&text[start..=offset])
                        .map_err(|_| invalid(raw));
                }
            }
            _ => {}
        }
    }
    Err(invalid(raw))
}

/// Remove a leading markdown code fence (``` / ```json) and trailing ``` if
/// both are present.
fn strip_fences(raw: &str) -> &str {
    let trimmed = raw.trim();
    let Some(without_open) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Drop an optional language tag on the same line.
    let after_tag = match without_open.find('\n') {
        Some(newline_at)
            if without_open[..newline_at]
                .chars()
                .all(|c| c.is_ascii_alphanumeric()) =>
        {
            &without_open[newline_at + 1..]
        }
        _ => without_open,
    };
    let without_close = after_tag.strip_suffix("```").unwrap_or(after_tag);
    without_close.trim()
}

fn invalid(raw: &str) -> cue_core::CueError {
    let preview: String = raw.chars().take(80).collect();
    cue_core::CueError::new(
        cue_core::PipelineStage::Analyze,
        "the gateway response was not valid JSON",
    )
    .because(format!("response began with: {preview:?}"))
    .remedy("retry, or use a model that follows JSON formatting instructions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json() {
        let v = parse_lenient_json(r#"{"a": 1}"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn parses_fenced_json_with_language_tag() {
        let v = parse_lenient_json("```json\n{\"title\": \"x\"}\n```").unwrap();
        assert_eq!(v["title"], "x");
    }

    #[test]
    fn extracts_object_from_surrounding_prose() {
        let v =
            parse_lenient_json("Here you go:\n{\"ok\": true}\nHope that helps!").unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn braces_inside_strings_do_not_confuse_depth() {
        let v = parse_lenient_json(r#"{"text": "curly } brace { and \" quote"}"#).unwrap();
        assert_eq!(v["text"], "curly } brace { and \" quote");
    }

    #[test]
    fn unterminated_object_is_an_error() {
        assert!(parse_lenient_json("{\"a\": 1").is_err());
        assert!(parse_lenient_json("no json at all").is_err());
    }
}
