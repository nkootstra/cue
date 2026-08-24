//! Audio extraction via ffmpeg: normalize any media input into a
//! predictable format for transcription.

use std::path::Path;

use cue_core::{CueError, PipelineStage};
use tracing::instrument;

/// Extraction parameters: 16 kHz mono s16 WAV is the lingua franca every
/// transcription backend accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioExtractOptions {
    pub sample_rate_hz: u32,
    pub channels: u32,
}

impl Default for AudioExtractOptions {
    fn default() -> Self {
        Self {
            sample_rate_hz: 16_000,
            channels: 1,
        }
    }
}

/// Extract and normalize the audio track of `input` into `output`.
///
/// Callers decide where the file lives (usually the content cache); this
/// function only guarantees the format.
#[instrument(skip_all, fields(input = %input.display()))]
pub async fn extract_audio(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    options: &AudioExtractOptions,
) -> cue_core::Result<()> {
    if !input.exists() {
        return Err(CueError::new(
            PipelineStage::Extract,
            format!("input file {} does not exist", input.display()),
        ));
    }

    let output = tokio::process::Command::new(ffmpeg)
        .args([
            "-y",
            "-v", "error",
            "-i",
        ])
        .arg(input)
        .args([
            "-vn",                    // drop video entirely
            "-acodec", "pcm_s16le",   // raw PCM for whisper-family models
            "-ar", &options.sample_rate_hz.to_string(),
            "-ac", &options.channels.to_string(),
        ])
        .arg(output)
        .output()
        .await
        .map_err(|e| {
            CueError::new(PipelineStage::Extract, "could not run ffmpeg")
                .because(e.to_string())
                .remedy("verify FFmpeg is installed and runnable with `cue doctor`")
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(extract_failure(input, &stderr));
    }

    Ok(())
}

/// Map common ffmpeg failure modes onto actionable errors.
fn extract_failure(input: &Path, stderr: &str) -> CueError {
    let stderr = stderr.trim();
    if stderr.contains("does not contain any stream") || stderr.contains("Invalid data") {
        CueError::new(
            PipelineStage::Extract,
            format!("{} is not readable as media", input.display()),
        )
        .because(stderr_tail(stderr))
        .remedy("the file may be corrupt or in an unsupported container")
    } else if stderr.contains("Output file") && stderr.contains("does not contain any stream") {
        CueError::new(
            PipelineStage::Extract,
            format!("{} contains no audio stream to transcribe", input.display()),
        )
        .remedy("cue can only process files that contain audio")
    } else {
        CueError::new(
            PipelineStage::Extract,
            format!("ffmpeg failed while extracting audio from {}", input.display()),
        )
        .because(stderr_tail(stderr))
    }
}

fn stderr_tail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(3);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::find_on_path;

    async fn ffmpeg_path() -> std::path::PathBuf {
        find_on_path("ffmpeg").expect("ffmpeg required for extraction tests")
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cue-extract-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Generate an MP4 with tone + test pattern via two lavfi inputs.
    async fn make_mp4(dir: &Path) -> std::path::PathBuf {
        let out = dir.join("input.mp4");
        let status = tokio::process::Command::new(find_on_path("ffmpeg").unwrap())
            .args(["-y", "-v", "error"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
            .args(["-f", "lavfi", "-i", "testsrc=duration=2:size=160x120:rate=15"])
            .args(["-shortest"])
            .arg(&out)
            .status()
            .await
            .unwrap();
        assert!(status.success());
        out
    }

    #[tokio::test]
    async fn extracts_normalized_wav_from_mp4() {
        let dir = temp_dir("mp4");
        let input = make_mp4(&dir).await;
        let output = dir.join("extracted.wav");

        extract_audio(&ffmpeg_path().await, &input, &output, &AudioExtractOptions::default())
            .await
            .unwrap();

        // Verify the result by re-inspecting it with ffprobe.
        let ffprobe = find_on_path("ffprobe").unwrap();
        let media = crate::probe::inspect(&ffprobe, &output).await.unwrap();
        assert!(media.has_audio());
        assert!(!media.has_video());
        assert_eq!(media.audio_streams[0].sample_rate_hz, 16_000);
        assert_eq!(media.audio_streams[0].channels, 1);
        assert_eq!(media.audio_streams[0].codec, "pcm_s16le");
    }

    #[tokio::test]
    async fn missing_input_is_stage_error() {
        let err = extract_audio(
            &ffmpeg_path().await,
            Path::new("/nonexistent/x.mp4"),
            Path::new("/tmp/out.wav"),
            &AudioExtractOptions::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.stage(), Some(PipelineStage::Extract));
    }

    #[tokio::test]
    async fn non_media_input_produces_actionable_error() {
        let dir = temp_dir("garbage");
        let fake = dir.join("fake.mp4");
        std::fs::write(&fake, b"definitely not media").unwrap();
        let output = dir.join("out.wav");

        let err = extract_audio(&ffmpeg_path().await, &fake, &output, &AudioExtractOptions::default())
            .await
            .unwrap_err();

        assert_eq!(err.stage(), Some(PipelineStage::Extract));
        let rendered = err.to_string();
        assert!(rendered.contains("not readable"), "{rendered}");
        assert!(!output.exists(), "no output should exist on failure");
    }

    #[test]
    fn failure_classifier_matches_ffmpeg_messages() {
        let invalid = extract_failure(Path::new("/x.mp4"), "Invalid data found when processing input");
        assert!(invalid.to_string().contains("not readable"));

        let generic = extract_failure(Path::new("/x.mp4"), "some other error");
        assert!(generic.to_string().contains("ffmpeg failed"));
    }
}
