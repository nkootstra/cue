//! Receipt-backed output verification shared by the CLI and batch recovery.

use std::path::Path;

use crate::run_contract::{ReceiptReadError, RunReceipt, TrackedFile};

pub(crate) struct VerificationResult {
    pub(crate) receipt: RunReceipt,
    pub(crate) diagnostics: Vec<VerificationDiagnostic>,
}

impl VerificationResult {
    pub(crate) fn is_valid(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub(crate) fn artifact_count(&self) -> usize {
        self.receipt.artifacts.len() + self.receipt.published_outputs.len()
    }
}

#[derive(serde::Serialize)]
pub(crate) struct VerificationDiagnostic {
    pub(crate) id: &'static str,
    pub(crate) path: String,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) expected_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) actual_digest: Option<String>,
}

pub(crate) fn verify_output(
    output_dir: &Path,
) -> std::result::Result<VerificationResult, ReceiptReadError> {
    let receipt = RunReceipt::read_for_verification(output_dir)?;
    let mut diagnostics = Vec::new();
    if let Some(diagnostic) = verify_tracked_file(output_dir, &receipt.source, TrackedKind::Source)
    {
        diagnostics.push(diagnostic);
    }
    for correction in &receipt.corrections {
        if let Some(diagnostic) =
            verify_tracked_file(output_dir, correction, TrackedKind::Correction)
        {
            diagnostics.push(diagnostic);
        }
    }
    for artifact in &receipt.artifacts {
        if let Some(diagnostic) = verify_tracked_file(output_dir, artifact, TrackedKind::Artifact) {
            diagnostics.push(diagnostic);
        }
    }
    for published in &receipt.published_outputs {
        if let Some(diagnostic) =
            verify_tracked_file(output_dir, published, TrackedKind::PublishedOutput)
        {
            diagnostics.push(diagnostic);
        }
    }
    Ok(VerificationResult {
        receipt,
        diagnostics,
    })
}

fn verify_tracked_file(
    output_dir: &Path,
    tracked: &TrackedFile,
    kind: TrackedKind,
) -> Option<VerificationDiagnostic> {
    let path = output_dir.join(&tracked.path);
    match crate::run_contract::is_regular_file(&path, kind.follows_symlinks()) {
        Ok(false) => {
            return Some(VerificationDiagnostic::tracked(
                kind.unsafe_id(),
                tracked,
                format!("{} is not a regular file", tracked.path),
                None,
            ));
        }
        Err(_) => {
            return Some(VerificationDiagnostic::tracked(
                kind.missing_id(),
                tracked,
                format!("{} is missing or unreadable", tracked.path),
                None,
            ));
        }
        Ok(true) => {}
    }
    match cue_cache::file_hash(&path) {
        Ok(actual) if actual == tracked.digest.value => None,
        Ok(actual) => Some(VerificationDiagnostic::tracked(
            kind.mismatch_id(),
            tracked,
            format!("{} does not match cue.run.json", tracked.path),
            Some(actual),
        )),
        Err(_) => Some(VerificationDiagnostic::tracked(
            kind.missing_id(),
            tracked,
            format!("{} is missing or unreadable", tracked.path),
            None,
        )),
    }
}

impl VerificationDiagnostic {
    pub(crate) fn operational(id: &'static str, message: String) -> Self {
        Self {
            id,
            path: crate::run_contract::RECEIPT_FILE.into(),
            message,
            expected_digest: None,
            actual_digest: None,
        }
    }

    fn tracked(
        id: &'static str,
        tracked: &TrackedFile,
        message: String,
        actual_digest: Option<String>,
    ) -> Self {
        Self {
            id,
            path: tracked.path.clone(),
            message,
            expected_digest: Some(tracked.digest.value.clone()),
            actual_digest,
        }
    }
}

#[derive(Clone, Copy)]
enum TrackedKind {
    Source,
    Correction,
    Artifact,
    PublishedOutput,
}

impl TrackedKind {
    const fn follows_symlinks(self) -> bool {
        !matches!(self, Self::Artifact | Self::PublishedOutput)
    }

    const fn mismatch_id(self) -> &'static str {
        match self {
            Self::Source => "CUE-VERIFY-SOURCE-MISMATCH",
            Self::Correction => "CUE-VERIFY-CORRECTION-MISMATCH",
            Self::Artifact => "CUE-VERIFY-ARTIFACT-MISMATCH",
            Self::PublishedOutput => "CUE-VERIFY-PUBLISHED-OUTPUT-MISMATCH",
        }
    }

    const fn missing_id(self) -> &'static str {
        match self {
            Self::Source => "CUE-VERIFY-SOURCE-MISSING",
            Self::Correction => "CUE-VERIFY-CORRECTION-MISSING",
            Self::Artifact => "CUE-VERIFY-ARTIFACT-MISSING",
            Self::PublishedOutput => "CUE-VERIFY-PUBLISHED-OUTPUT-MISSING",
        }
    }

    const fn unsafe_id(self) -> &'static str {
        match self {
            Self::Source => "CUE-VERIFY-SOURCE-UNSAFE",
            Self::Correction => "CUE-VERIFY-CORRECTION-UNSAFE",
            Self::Artifact => "CUE-VERIFY-ARTIFACT-UNSAFE",
            Self::PublishedOutput => "CUE-VERIFY-PUBLISHED-OUTPUT-UNSAFE",
        }
    }
}
