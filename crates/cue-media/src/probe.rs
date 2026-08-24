//! Media inspection via ffprobe.

use std::path::Path;
use std::process::Stdio;

use cue_core::media::{AudioStream, Media, VideoStream};
use cue_core::{CueError, PipelineStage};
use serde::Deserialize;

/// Inspect a media file with ffprobe and map it onto the domain model.
pub async fn inspect(ffprobe: &Path, input: &Path) -> cue_core::Result<Media> {
    if !input.exists() {
        return Err(CueError::new(
            PipelineStage::Inspect,
            format!("input file {} does not exist", input.display()),
        )
        .remedy("check the path and try again"));
    }

    let output = tokio::process::Command::new(ffprobe)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|e| {
            CueError::new(PipelineStage::Inspect, "could not run ffprobe").because(e.to_string())
        })?;

    if !output.status.success() {
        return Err(CueError::new(
            PipelineStage::Inspect,
            format!("ffprobe could not read {}", input.display()),
        )
        .because(exit_reason(output.status.code()))
        .remedy("the file may be corrupt or not a supported media file"));
    }

    let parsed: FfProbeOutput = serde_json::from_slice(&output.stdout).map_err(|e| {
        CueError::new(PipelineStage::Inspect, "ffprobe returned unreadable output")
            .because(e.to_string())
    })?;

    parsed.into_media(input).ok_or_else(|| {
        CueError::new(
            PipelineStage::Inspect,
            format!("{} has no readable duration or streams", input.display()),
        )
        .remedy("the file may be empty or corrupt")
    })
}

fn exit_reason(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("exited with status {c}"),
        None => "was terminated".to_string(),
    }
}

// Typed subset of ffprobe's JSON output.
#[derive(Debug, Deserialize)]
struct FfProbeOutput {
    #[serde(default)]
    streams: Vec<FfStream>,
    #[serde(default)]
    format: Option<FfFormat>,
}

#[derive(Debug, Deserialize)]
struct FfFormat {
    #[serde(default)]
    format_name: Option<String>,
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfStream {
    index: u32,
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    codec_name: Option<String>,
    #[serde(default)]
    sample_rate: Option<String>,
    #[serde(default)]
    channels: Option<u32>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
}

impl FfProbeOutput {
    fn into_media(self, path: &Path) -> Option<Media> {
        let duration_ms = self
            .format
            .as_ref()
            .and_then(|f| f.duration.as_deref())
            .and_then(parse_seconds_to_ms)?;

        let mut audio_streams = Vec::new();
        let mut video_streams = Vec::new();
        for stream in &self.streams {
            match stream.codec_type.as_deref() {
                Some("audio") => audio_streams.push(AudioStream {
                    index: stream.index,
                    codec: stream.codec_name.clone().unwrap_or_default(),
                    sample_rate_hz: stream
                        .sample_rate
                        .as_deref()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                    channels: stream.channels.unwrap_or(0),
                }),
                Some("video") => video_streams.push(VideoStream {
                    index: stream.index,
                    codec: stream.codec_name.clone().unwrap_or_default(),
                    width: stream.width.unwrap_or(0),
                    height: stream.height.unwrap_or(0),
                    frame_rate: stream.avg_frame_rate.clone().unwrap_or_default(),
                }),
                _ => {}
            }
        }

        // An empty/undecodable file yields no duration at all; surface that
        // as an error rather than a zero-duration media object.
        Some(Media {
            schema_version: 1,
            path: path.to_path_buf(),
            duration_ms,
            format: self.format.and_then(|f| f.format_name).unwrap_or_default(),
            audio_streams,
            video_streams,
        })
    }
}

fn parse_seconds_to_ms(seconds: &str) -> Option<u64> {
    let value: f64 = seconds.parse().ok()?;
    if value < 0.0 || !value.is_finite() {
        return None;
    }
    Some((value * 1000.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::find_on_path;

    /// Generate a tiny WAV fixture with ffmpeg into the target temp dir so
    /// tests never commit binary blobs to the repository.
    async fn make_wav(dir: &Path, seconds: u32) -> std::path::PathBuf {
        let out = dir.join(format!("tone-{seconds}s.wav"));
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!("sine=frequency=440:duration={seconds}"),
                "-ar",
                "16000",
                "-ac",
                "1",
            ])
            .arg(&out)
            .status()
            .await
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg failed to create {out:?}");
        out
    }

    fn ffprobe_path() -> Option<std::path::PathBuf> {
        find_on_path("ffprobe")
    }

    #[tokio::test]
    async fn inspects_generated_wav() {
        let Some(ffprobe) = ffprobe_path() else {
            return;
        };
        let dir = std::env::temp_dir().join("cue-test-fixtures");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let wav = make_wav(&dir, 1).await;

        let media = inspect(&ffprobe, &wav).await.unwrap();
        assert!(media.has_audio());
        assert!(!media.has_video());
        assert_eq!(media.audio_streams[0].codec, "pcm_s16le");
        assert_eq!(media.audio_streams[0].sample_rate_hz, 16_000);
        assert_eq!(media.audio_streams[0].channels, 1);
        // Sine generation is slightly imprecise; allow tolerance.
        assert!(
            (900..=1100).contains(&media.duration_ms),
            "duration was {}ms",
            media.duration_ms
        );
    }

    #[tokio::test]
    async fn missing_file_is_actionable_error() {
        let Some(ffprobe) = ffprobe_path() else {
            return;
        };
        let err = inspect(&ffprobe, Path::new("/nonexistent/file.mp4"))
            .await
            .unwrap_err();
        assert_eq!(err.stage(), Some(PipelineStage::Inspect));
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[tokio::test]
    async fn text_file_is_not_valid_media() {
        let Some(ffprobe) = ffprobe_path() else {
            return;
        };
        let dir = std::env::temp_dir().join("cue-test-fixtures");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let fake = dir.join("not-media.txt");
        tokio::fs::write(&fake, b"this is not a media file")
            .await
            .unwrap();

        let err = inspect(&ffprobe, &fake).await.unwrap_err();
        assert_eq!(err.stage(), Some(PipelineStage::Inspect));
    }

    #[test]
    fn parses_duration_strings() {
        assert_eq!(parse_seconds_to_ms("5.123000"), Some(5123));
        assert_eq!(parse_seconds_to_ms("0"), Some(0));
        assert_eq!(parse_seconds_to_ms("-1.0"), None);
        assert_eq!(parse_seconds_to_ms("NaN"), None);
        assert_eq!(parse_seconds_to_ms(""), None);
    }

    #[test]
    fn maps_probe_json_subset() {
        let json = r#"{
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "h264",
                 "width": 640, "height": 360, "avg_frame_rate": "30/1"},
                {"index": 1, "codec_type": "audio", "codec_name": "aac",
                 "sample_rate": "48000", "channels": 2},
                {"index": 2, "codec_type": "data"}
            ],
            "format": {"format_name": "mov,mp4,m4a", "duration": "61.500000"}
        }"#;
        let parsed: FfProbeOutput = serde_json::from_str(json).unwrap();
        let media = parsed.into_media(Path::new("/tmp/x.mp4")).unwrap();
        assert_eq!(media.duration_ms, 61_500);
        assert_eq!(media.format, "mov,mp4,m4a");
        assert_eq!(media.video_streams.len(), 1);
        assert_eq!(media.audio_streams.len(), 1);
        assert_eq!(media.audio_streams[0].channels, 2);
    }

    #[test]
    fn missing_duration_yields_none() {
        let json = r#"{"streams": [], "format": {"format_name": "mp3"}}"#;
        let parsed: FfProbeOutput = serde_json::from_str(json).unwrap();
        assert!(parsed.into_media(Path::new("/x")).is_none());
    }
}
