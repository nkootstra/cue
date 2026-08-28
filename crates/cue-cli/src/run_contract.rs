//! Versioned completion receipt shared by pipeline publication and verification.

use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cue_core::{CueError, Result};

pub(crate) const RECEIPT_FILE: &str = "cue.run.json";
pub(crate) const SCHEMA_VERSION: u32 = 1;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const LOCK_FILE: &str = ".cue.lock";

pub(crate) struct OutputLock {
    _file: std::fs::File,
}

impl OutputLock {
    pub(crate) fn acquire(output_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(output_dir).map_err(|error| {
            CueError::new(
                cue_core::PipelineStage::Render,
                format!("could not create output directory {}", output_dir.display()),
            )
            .because(error.to_string())
        })?;
        let path = output_dir.join(LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                CueError::new(
                    cue_core::PipelineStage::Render,
                    format!("could not open output lock {}", path.display()),
                )
                .because(error.to_string())
            })?;
        file.lock().map_err(|error| {
            CueError::new(
                cue_core::PipelineStage::Render,
                format!("could not lock output directory {}", output_dir.display()),
            )
            .because(error.to_string())
        })?;
        Ok(Self { _file: file })
    }
}

pub(crate) fn tracked_reference(output_dir: &Path, path: &Path) -> Result<String> {
    if output_dir.to_str().is_none() || path.to_str().is_none() {
        return Err(CueError::general(
            "run receipts do not support non-UTF-8 source or output paths",
        )
        .remedy("rename the source/output path to valid UTF-8 and run cue again"));
    }
    Ok(crate::corrections::manifest_reference(output_dir, path))
}

pub(crate) fn configuration_snapshot(
    config: &cue_core::Config,
    language: Option<&str>,
) -> ConfigurationSnapshot {
    ConfigurationSnapshot {
        language: language.map(str::to_owned),
        transcription: config.transcription.clone(),
        normalization: NormalizationSnapshot {
            provider: config.normalization.provider.clone(),
            ollama_url: sanitize_endpoint(&config.normalization.ollama_url),
            styling: config.normalization.styling.clone(),
            structure: config.normalization.structure.clone(),
            context: config.normalization.context.clone(),
        },
        subtitles: config.subtitles.clone(),
        analysis: config.analysis.clone(),
        llm: config.llm.as_ref().map(|llm| LlmSnapshot {
            base_url: sanitize_endpoint(&llm.base_url),
            model: llm.model.clone(),
        }),
    }
}

pub(crate) fn sanitize_endpoint(endpoint: &str) -> String {
    let Ok(mut url) = url::Url::parse(endpoint) else {
        return "<invalid-url>".into();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_string()
}

pub(crate) fn endpoint_is_remote(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint) else {
        return true;
    };
    match url.host() {
        Some(url::Host::Domain(host)) => !host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => !address.is_loopback(),
        Some(url::Host::Ipv6(address)) => {
            !address.is_loopback()
                && !address
                    .to_ipv4_mapped()
                    .is_some_and(|address| address.is_loopback())
        }
        None => true,
    }
}

pub(crate) fn invalidate(output_dir: &Path) -> Result<()> {
    let path = output_dir.join(RECEIPT_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CueError::new(
            cue_core::PipelineStage::Render,
            format!("could not remove stale run receipt {}", path.display()),
        )
        .because(error.to_string())),
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct RunReceipt {
    pub(crate) schema_version: u32,
    pub(crate) cue_version: String,
    pub(crate) mode: ProcessModeName,
    pub(crate) source: TrackedFile,
    pub(crate) configuration: ConfigurationSnapshot,
    pub(crate) providers: Vec<ProviderIdentity>,
    pub(crate) stages: Vec<StageRecord>,
    pub(crate) warnings: Vec<String>,
    pub(crate) remote_data_usage: RemoteDataUsage,
    pub(crate) corrections: Vec<TrackedFile>,
    pub(crate) artifacts: Vec<TrackedFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProcessModeName {
    Full,
    TranscriptOnly,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct TrackedFile {
    pub(crate) path: String,
    pub(crate) digest: Digest,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Digest {
    pub(crate) algorithm: String,
    pub(crate) value: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProviderIdentity {
    pub(crate) stage: cue_core::PipelineStage,
    pub(crate) provider: String,
    pub(crate) model: Option<String>,
    pub(crate) endpoint: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct StageRecord {
    pub(crate) stage: cue_core::PipelineStage,
    pub(crate) status: StageStatus,
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum StageStatus {
    Executed,
    Cached,
    Skipped,
    Degraded,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct RemoteDataUsage {
    pub(crate) normalized_text_sent_to_remote_in_current_run: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ConfigurationSnapshot {
    language: Option<String>,
    transcription: cue_core::config::TranscriptionConfig,
    normalization: NormalizationSnapshot,
    subtitles: cue_core::config::SubtitlesConfig,
    analysis: cue_core::config::AnalysisConfig,
    llm: Option<LlmSnapshot>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct NormalizationSnapshot {
    provider: String,
    ollama_url: String,
    styling: String,
    structure: String,
    context: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct LlmSnapshot {
    base_url: String,
    model: String,
}

#[derive(Debug)]
pub(crate) enum ReceiptReadError {
    Missing { path: PathBuf },
    Unreadable { path: PathBuf, reason: String },
    Malformed { path: PathBuf, reason: String },
    UnsupportedSchema { version: u32 },
    Invalid { reason: String },
}

impl ReceiptReadError {
    pub(crate) const fn diagnostic_id(&self) -> &'static str {
        match self {
            Self::Missing { .. } => "CUE-VERIFY-RECEIPT-MISSING",
            Self::Unreadable { .. } => "CUE-VERIFY-RECEIPT-UNREADABLE",
            Self::Malformed { .. } => "CUE-VERIFY-RECEIPT-MALFORMED",
            Self::UnsupportedSchema { .. } => "CUE-VERIFY-SCHEMA-UNSUPPORTED",
            Self::Invalid { .. } => "CUE-VERIFY-RECEIPT-INVALID",
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::Missing { path } => format!("run receipt {} is missing", path.display()),
            Self::Unreadable { path, reason } => {
                format!("could not read {}: {reason}", path.display())
            }
            Self::Malformed { path, reason } => {
                format!("could not parse {}: {reason}", path.display())
            }
            Self::UnsupportedSchema { version } => format!(
                "unsupported run receipt schema version {version}; this cue supports schema version {SCHEMA_VERSION}"
            ),
            Self::Invalid { reason } => reason.clone(),
        }
    }
}

impl RunReceipt {
    pub(crate) fn read_for_verification(
        output_dir: &Path,
    ) -> std::result::Result<Self, ReceiptReadError> {
        let path = output_dir.join(RECEIPT_FILE);
        let file = std::fs::File::open(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ReceiptReadError::Missing { path: path.clone() }
            } else {
                ReceiptReadError::Unreadable {
                    path: path.clone(),
                    reason: error.to_string(),
                }
            }
        })?;
        let receipt: Self =
            serde_json::from_reader(std::io::BufReader::new(file)).map_err(|error| {
                ReceiptReadError::Malformed {
                    path,
                    reason: error.to_string(),
                }
            })?;
        if receipt.schema_version != SCHEMA_VERSION {
            return Err(ReceiptReadError::UnsupportedSchema {
                version: receipt.schema_version,
            });
        }
        receipt
            .validate()
            .map_err(|error| ReceiptReadError::Invalid {
                reason: error.to_string().trim().to_owned(),
            })?;
        Ok(receipt)
    }

    pub(crate) fn publish<S: AsRef<str>>(
        mut self,
        output_dir: &Path,
        artifact_names: &[S],
    ) -> Result<()> {
        (|| {
            self.verify_inputs_current(output_dir)?;
            let artifact_names = artifact_names
                .iter()
                .map(AsRef::as_ref)
                .collect::<std::collections::BTreeSet<_>>();
            self.artifacts = artifact_names
                .into_iter()
                .map(|name| {
                    validate_artifact_path(name)?;
                    let path = output_dir.join(name);
                    require_regular_file(&path, false)?;
                    TrackedFile::from_path(name, &path)
                })
                .collect::<Result<Vec<_>>>()?;
            self.validate()?;
            let mut bytes = serde_json::to_vec_pretty(&self).map_err(|error| {
                CueError::general("could not serialize run receipt").because(error.to_string())
            })?;
            bytes.push(b'\n');
            write_atomic(&output_dir.join(RECEIPT_FILE), &bytes)
        })()
        .map_err(|error: CueError| error.at_stage(cue_core::PipelineStage::Render))
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(CueError::general(format!(
                "unsupported run receipt schema version {}",
                self.schema_version
            ))
            .remedy(format!("this cue supports schema version {SCHEMA_VERSION}")));
        }
        if self.cue_version.trim().is_empty() {
            return Err(CueError::general(
                "run receipt contains an empty cue version",
            ));
        }
        if self.source.path.is_empty() {
            return Err(CueError::general(
                "run receipt contains an empty source path",
            ));
        }
        validate_digest(&self.source.digest)?;
        let mut paths = std::collections::HashSet::new();
        for artifact in &self.artifacts {
            validate_artifact_path(&artifact.path)?;
            validate_digest(&artifact.digest)?;
            if !paths.insert(artifact.path.as_str()) {
                return Err(CueError::general(format!(
                    "run receipt contains duplicate artifact {}",
                    artifact.path
                )));
            }
        }
        for correction in &self.corrections {
            if correction.path.is_empty() {
                return Err(CueError::general(
                    "run receipt contains an empty correction path",
                ));
            }
            validate_digest(&correction.digest)?;
        }
        for required in ["transcript.json", "transcript.txt"] {
            if !paths.contains(required) {
                return Err(CueError::general(format!(
                    "run receipt is missing required artifact {required}"
                )));
            }
        }
        let mut stages = std::collections::HashSet::new();
        for stage in &self.stages {
            if !stages.insert(stage.stage) {
                return Err(CueError::general(format!(
                    "run receipt contains duplicate stage {}",
                    stage.stage
                )));
            }
            let valid_status = match stage.stage {
                cue_core::PipelineStage::Inspect | cue_core::PipelineStage::Render => {
                    stage.status == StageStatus::Executed
                }
                cue_core::PipelineStage::Extract | cue_core::PipelineStage::Transcribe => {
                    matches!(stage.status, StageStatus::Executed | StageStatus::Cached)
                }
                cue_core::PipelineStage::Normalize => matches!(
                    stage.status,
                    StageStatus::Executed | StageStatus::Cached | StageStatus::Skipped
                ),
                cue_core::PipelineStage::Analyze => matches!(
                    stage.status,
                    StageStatus::Executed
                        | StageStatus::Cached
                        | StageStatus::Skipped
                        | StageStatus::Degraded
                ),
            };
            if !valid_status {
                return Err(CueError::general(format!(
                    "run receipt contains invalid status {:?} for stage {}",
                    stage.status, stage.stage
                )));
            }
        }
        let required_stages: &[cue_core::PipelineStage] = match self.mode {
            ProcessModeName::Full => &cue_core::PipelineStage::ALL,
            ProcessModeName::TranscriptOnly => &[
                cue_core::PipelineStage::Inspect,
                cue_core::PipelineStage::Extract,
                cue_core::PipelineStage::Transcribe,
                cue_core::PipelineStage::Render,
            ],
        };
        for stage in required_stages {
            if !stages.contains(stage) {
                return Err(CueError::general(format!(
                    "run receipt is missing required stage {stage}"
                )));
            }
        }
        if stages.len() != required_stages.len() {
            return Err(CueError::general(
                "run receipt contains stages that do not apply to its mode",
            ));
        }

        let mut provider_stages = std::collections::HashSet::new();
        for provider in &self.providers {
            if provider.provider.trim().is_empty() {
                return Err(CueError::general(
                    "run receipt contains an empty provider identity",
                ));
            }
            if !provider_stages.insert(provider.stage) {
                return Err(CueError::general(format!(
                    "run receipt contains duplicate provider for stage {}",
                    provider.stage
                )));
            }
            if !stages.contains(&provider.stage)
                || provider.stage == cue_core::PipelineStage::Render
            {
                return Err(CueError::general(format!(
                    "run receipt contains provider for inapplicable stage {}",
                    provider.stage
                )));
            }
        }
        let required_provider_stages = [
            cue_core::PipelineStage::Inspect,
            cue_core::PipelineStage::Extract,
            cue_core::PipelineStage::Transcribe,
        ];
        for stage in required_provider_stages {
            if !provider_stages.contains(&stage) {
                return Err(CueError::general(format!(
                    "run receipt is missing provider for stage {stage}"
                )));
            }
        }
        if self.mode == ProcessModeName::Full
            && !provider_stages.contains(&cue_core::PipelineStage::Normalize)
        {
            return Err(CueError::general(
                "run receipt is missing provider for stage normalize",
            ));
        }
        let analysis_status = self
            .stages
            .iter()
            .find(|stage| stage.stage == cue_core::PipelineStage::Analyze)
            .map(|stage| stage.status);
        if analysis_status.is_some_and(|status| status != StageStatus::Skipped)
            && !provider_stages.contains(&cue_core::PipelineStage::Analyze)
        {
            return Err(CueError::general(
                "run receipt is missing provider for stage analyze",
            ));
        }
        Ok(())
    }

    fn verify_inputs_current(&self, output_dir: &Path) -> Result<()> {
        for (kind, tracked) in std::iter::once(("source", &self.source)).chain(
            self.corrections
                .iter()
                .map(|tracked| ("correction", tracked)),
        ) {
            let path = output_dir.join(&tracked.path);
            require_regular_file(&path, true)?;
            let actual = cue_cache::file_hash(&path)?;
            if actual != tracked.digest.value {
                return Err(CueError::general(format!(
                    "{kind} {} changed while cue was processing it",
                    tracked.path
                ))
                .remedy("run cue again after the file stops changing"));
            }
        }
        Ok(())
    }
}

impl TrackedFile {
    pub(crate) fn from_path(reference: impl Into<String>, path: &Path) -> Result<Self> {
        Ok(Self {
            path: reference.into(),
            digest: Digest {
                algorithm: "blake3".into(),
                value: cue_cache::file_hash(path)?,
            },
        })
    }

    pub(crate) fn from_digest(reference: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            path: reference.into(),
            digest: Digest {
                algorithm: "blake3".into(),
                value: value.into(),
            },
        }
    }
}

impl StageRecord {
    pub(crate) fn new(
        stage: cue_core::PipelineStage,
        status: StageStatus,
        detail: Option<String>,
    ) -> Self {
        Self {
            stage,
            status,
            detail,
        }
    }
}

fn validate_digest(digest: &Digest) -> Result<()> {
    if digest.algorithm != "blake3"
        || digest.value.len() != 64
        || !digest
            .value
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(CueError::general(
            "run receipt contains an invalid BLAKE3 digest",
        ));
    }
    Ok(())
}

fn validate_artifact_path(path: &str) -> Result<()> {
    let path = Path::new(path);
    let mut components = path.components();
    let valid = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && path != Path::new(RECEIPT_FILE);
    if !valid {
        return Err(CueError::general(format!(
            "run receipt contains unsafe artifact path {}",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn is_regular_file(path: &Path, follow_symlinks: bool) -> std::io::Result<bool> {
    let metadata = if follow_symlinks {
        std::fs::metadata(path)?
    } else {
        std::fs::symlink_metadata(path)?
    };
    Ok(metadata.file_type().is_file() && (follow_symlinks || !metadata.file_type().is_symlink()))
}

fn require_regular_file(path: &Path, follow_symlinks: bool) -> Result<()> {
    match is_regular_file(path, follow_symlinks) {
        Ok(true) => Ok(()),
        Ok(false) => Err(CueError::general(format!(
            "tracked path {} is not a regular file",
            path.display()
        ))),
        Err(error) => Err(CueError::general(format!(
            "could not inspect tracked path {}",
            path.display()
        ))
        .because(error.to_string())),
    }
}

fn write_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CueError::general(format!(
            "could not determine the parent of {}",
            path.display()
        ))
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CueError::general("could not construct run receipt temporary file name"))?;
    loop {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(CueError::general(format!(
                    "could not create a temporary run receipt in {}",
                    parent.display()
                ))
                .because(error.to_string()));
            }
        };
        if let Err(error) = file.write_all(content).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&temp_path);
            return Err(
                CueError::general(format!("could not write {}", path.display()))
                    .because(error.to_string()),
            );
        }
        drop(file);
        match std::fs::rename(&temp_path, path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                std::fs::remove_file(path).map_err(|remove_error| {
                    let _ = std::fs::remove_file(&temp_path);
                    CueError::general(format!("could not replace {}", path.display()))
                        .because(remove_error.to_string())
                })?;
                std::fs::rename(&temp_path, path).map_err(|rename_error| {
                    let _ = std::fs::remove_file(&temp_path);
                    CueError::general(format!("could not publish {}", path.display()))
                        .because(rename_error.to_string())
                })?;
                return Ok(());
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temp_path);
                return Err(
                    CueError::general(format!("could not publish {}", path.display()))
                        .because(error.to_string()),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_receipt(source: &Path) -> RunReceipt {
        RunReceipt {
            schema_version: SCHEMA_VERSION,
            cue_version: "test".into(),
            mode: ProcessModeName::Full,
            source: TrackedFile::from_path("../lesson.mp4", source).unwrap(),
            configuration: configuration_snapshot(&cue_core::Config::default(), None),
            providers: vec![
                ProviderIdentity {
                    stage: cue_core::PipelineStage::Inspect,
                    provider: "ffprobe".into(),
                    model: None,
                    endpoint: None,
                },
                ProviderIdentity {
                    stage: cue_core::PipelineStage::Extract,
                    provider: "ffmpeg".into(),
                    model: None,
                    endpoint: None,
                },
                ProviderIdentity {
                    stage: cue_core::PipelineStage::Transcribe,
                    provider: "faster-whisper".into(),
                    model: Some("test".into()),
                    endpoint: None,
                },
                ProviderIdentity {
                    stage: cue_core::PipelineStage::Normalize,
                    provider: "s1".into(),
                    model: Some("test".into()),
                    endpoint: Some("http://localhost:11434".into()),
                },
            ],
            stages: cue_core::PipelineStage::ALL
                .into_iter()
                .map(|stage| {
                    let status = if stage == cue_core::PipelineStage::Analyze {
                        StageStatus::Skipped
                    } else {
                        StageStatus::Executed
                    };
                    StageRecord::new(stage, status, None)
                })
                .collect(),
            warnings: Vec::new(),
            remote_data_usage: RemoteDataUsage {
                normalized_text_sent_to_remote_in_current_run: None,
            },
            corrections: Vec::new(),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn publish_hashes_final_artifacts_and_atomically_replaces_the_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("lesson.mp4");
        let output = temp.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(&source, b"media").unwrap();
        std::fs::write(output.join("transcript.json"), b"{}\n").unwrap();
        std::fs::write(output.join("transcript.txt"), b"final transcript\n").unwrap();
        std::fs::write(output.join(RECEIPT_FILE), b"stale\n").unwrap();

        let receipt = full_receipt(&source);

        receipt
            .publish(&output, &["transcript.json", "transcript.txt"])
            .unwrap();

        let published = RunReceipt::read_for_verification(&output).unwrap();
        assert_eq!(published.artifacts.len(), 2);
        assert_eq!(published.artifacts[1].path, "transcript.txt");
        assert_eq!(
            published.artifacts[1].digest.value,
            cue_cache::file_hash(&output.join("transcript.txt")).unwrap()
        );
        assert!(
            !std::fs::read_to_string(output.join(RECEIPT_FILE))
                .unwrap()
                .contains("stale")
        );
        assert!(std::fs::read_dir(&output).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn publish_records_each_artifact_once() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("lesson.mp4");
        let output = temp.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(&source, b"media").unwrap();
        std::fs::write(output.join("transcript.json"), b"{}\n").unwrap();
        std::fs::write(output.join("transcript.txt"), b"transcript\n").unwrap();

        let receipt = full_receipt(&source);

        receipt
            .publish(
                &output,
                &["transcript.json", "transcript.txt", "transcript.txt"],
            )
            .unwrap();

        assert_eq!(
            RunReceipt::read_for_verification(&output)
                .unwrap()
                .artifacts
                .len(),
            2
        );
    }

    #[test]
    fn publish_failures_are_attributed_to_render() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("lesson.mp4");
        let output = temp.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(&source, b"media").unwrap();

        let receipt = full_receipt(&source);

        let error = receipt.publish(&output, &["missing.txt"]).unwrap_err();

        assert_eq!(error.stage(), Some(cue_core::PipelineStage::Render));
        assert!(!output.join(RECEIPT_FILE).exists());
    }

    #[test]
    fn semantic_validation_rejects_incomplete_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("lesson.mp4");
        std::fs::write(&source, b"media").unwrap();

        let error = full_receipt(&source).validate().unwrap_err();

        assert!(error.to_string().contains("transcript.json"), "{error}");
    }

    #[test]
    fn semantic_validation_rejects_impossible_stage_statuses() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("lesson.mp4");
        std::fs::write(&source, b"media").unwrap();
        let mut receipt = full_receipt(&source);
        receipt.artifacts = vec![
            TrackedFile::from_digest("transcript.json", "0".repeat(64)),
            TrackedFile::from_digest("transcript.txt", "0".repeat(64)),
        ];
        receipt
            .stages
            .iter_mut()
            .find(|stage| stage.stage == cue_core::PipelineStage::Render)
            .unwrap()
            .status = StageStatus::Skipped;

        let error = receipt.validate().unwrap_err();

        assert!(error.to_string().contains("invalid status"), "{error}");
    }

    #[test]
    fn semantic_validation_requires_provider_identities() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("lesson.mp4");
        std::fs::write(&source, b"media").unwrap();
        let mut receipt = full_receipt(&source);
        receipt.artifacts = vec![
            TrackedFile::from_digest("transcript.json", "0".repeat(64)),
            TrackedFile::from_digest("transcript.txt", "0".repeat(64)),
        ];
        receipt.providers.clear();

        let error = receipt.validate().unwrap_err();

        assert!(error.to_string().contains("missing provider"), "{error}");
    }

    #[test]
    fn publish_rejects_sources_that_changed_during_processing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("lesson.mp4");
        let output = temp.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(&source, b"original").unwrap();
        let receipt = full_receipt(&source);
        std::fs::write(&source, b"changed").unwrap();
        std::fs::write(output.join("transcript.json"), b"{}\n").unwrap();
        std::fs::write(output.join("transcript.txt"), b"transcript\n").unwrap();

        let error = receipt
            .publish(&output, &["transcript.json", "transcript.txt"])
            .unwrap_err();

        assert_eq!(error.stage(), Some(cue_core::PipelineStage::Render));
        assert!(
            error
                .to_string()
                .contains("changed while cue was processing")
        );
        assert!(!output.join(RECEIPT_FILE).exists());
    }

    #[test]
    fn output_lock_serializes_renderers() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("lesson.cue");
        let first = OutputLock::acquire(&output).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let waiting_output = output.clone();
        let waiter = std::thread::spawn(move || {
            let lock = OutputLock::acquire(&waiting_output).unwrap();
            sender.send(lock).unwrap();
        });

        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err()
        );
        drop(first);
        let second = receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("second renderer acquires lock after first completes");
        drop(second);
        waiter.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn tracked_references_reject_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;

        let output = PathBuf::from("lesson.cue");
        let source = PathBuf::from(std::ffi::OsString::from_vec(vec![b'm', 0xff]));

        let error = tracked_reference(&output, &source).unwrap_err();

        assert!(error.to_string().contains("non-UTF-8"), "{error}");
    }

    #[test]
    fn configuration_snapshot_records_effective_values_without_url_credentials() {
        let mut config = cue_core::Config::default();
        config.llm = Some(cue_core::config::LlmConfig {
            base_url: "https://user:pass@gateway.example/v1?token=secret#fragment".into(),
            model: "model-x".into(),
            api_key_env: "CUE_SECRET".into(),
        });

        let snapshot = configuration_snapshot(&config, Some("en"));
        let snapshot = serde_json::to_value(snapshot).unwrap();

        assert_eq!(snapshot["language"], "en");
        assert_eq!(
            snapshot["transcription"]["model"],
            config.transcription.model
        );
        assert_eq!(snapshot["llm"]["base_url"], "https://gateway.example/v1");
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("user"));
        assert!(!serialized.contains("pass"));
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("CUE_SECRET"));
    }

    #[test]
    fn remote_endpoint_detection_recognizes_loopback_hosts() {
        assert!(!endpoint_is_remote("http://localhost:11434"));
        assert!(!endpoint_is_remote("http://127.0.0.1:11434"));
        assert!(!endpoint_is_remote("http://[::1]:11434"));
        assert!(!endpoint_is_remote("http://[::ffff:127.0.0.1]:11434"));
        assert!(endpoint_is_remote("https://gateway.example/v1"));
        assert!(endpoint_is_remote("not a URL"));
    }
}
