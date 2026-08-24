//! Provider-independent media metadata.
//!
//! Produced by inspecting a file with ffprobe; consumed by the pipeline to
//! decide how to extract audio and to validate inputs early.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const MEDIA_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub schema_version: u32,
    pub path: PathBuf,
    /// Container duration in milliseconds.
    pub duration_ms: u64,
    /// Container format name as reported by ffprobe (e.g. "mov,mp4,m4a").
    pub format: String,
    pub audio_streams: Vec<AudioStream>,
    pub video_streams: Vec<VideoStream>,
}

impl Media {
    pub fn has_audio(&self) -> bool {
        !self.audio_streams.is_empty()
    }

    pub fn has_video(&self) -> bool {
        !self.video_streams.is_empty()
    }

    /// The stream audio should be extracted from: the first audio stream.
    pub fn primary_audio(&self) -> Option<&AudioStream> {
        self.audio_streams.first()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioStream {
    pub index: u32,
    /// Codec name as reported by ffprobe (e.g. "aac").
    pub codec: String,
    pub sample_rate_hz: u32,
    pub channels: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoStream {
    pub index: u32,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Average frame rate as reported ("30000/1001"); kept raw because
    /// consumers differ on whether they want exact fractions.
    pub frame_rate: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_media() -> Media {
        Media {
            schema_version: MEDIA_SCHEMA_VERSION,
            path: PathBuf::from("/tmp/video.mp4"),
            duration_ms: 61_500,
            format: "mov,mp4,m4a".into(),
            audio_streams: vec![AudioStream {
                index: 1,
                codec: "aac".into(),
                sample_rate_hz: 48_000,
                channels: 2,
            }],
            video_streams: vec![VideoStream {
                index: 0,
                codec: "h264".into(),
                width: 1920,
                height: 1080,
                frame_rate: "30000/1001".into(),
            }],
        }
    }

    #[test]
    fn serializes_with_schema_version() {
        let json = serde_json::to_string(&sample_media()).unwrap();
        assert!(json.contains(r#""schema_version":1"#), "{json}");
        assert!(json.contains(r#""duration_ms":61500"#), "{json}");
    }

    #[test]
    fn deserializes_old_files_without_schema_version() {
        // Forward compatibility: files written before versioning existed
        // still load, defaulting to version 1.
        let json = r#"{
            "path": "/tmp/a.mp4",
            "duration_ms": 1000,
            "format": "mp4",
            "audio_streams": [],
            "video_streams": []
        }"#;
        let media: Media = serde_json::from_str(json).unwrap();
        assert_eq!(media.schema_version, MEDIA_SCHEMA_VERSION);
    }

    #[test]
    fn stream_helpers() {
        let media = sample_media();
        assert!(media.has_audio());
        assert!(media.has_video());
        assert_eq!(media.primary_audio().unwrap().codec, "aac");

        let mut silent = media.clone();
        silent.audio_streams.clear();
        assert!(!silent.has_audio());
        assert!(silent.primary_audio().is_none());
    }
}
