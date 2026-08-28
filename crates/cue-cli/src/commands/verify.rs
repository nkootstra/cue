//! `cue verify` — validate a completed output against its run receipt.

use std::path::Path;

use crate::cli::VerifyArgs;
use crate::render::println_line;
use crate::run_contract::{ReceiptReadError, RunReceipt, TrackedFile};

pub fn run(args: VerifyArgs) -> i32 {
    let output_label = args.output.display().to_string();
    let result = crate::commands::correct::resolve_output_dir(&args.output)
        .map_err(|error| OperationalError {
            id: "CUE-VERIFY-OUTPUT-INVALID",
            message: error.to_string().trim().to_owned(),
        })
        .and_then(|output_dir| {
            verify_output(&output_dir).map_err(|error| OperationalError {
                id: error.diagnostic_id(),
                message: error.message(),
            })
        });

    if args.json {
        let report = match result {
            Ok(report) => report,
            Err(error) => VerifyReport::failure(output_label, error),
        };
        return match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                println_line(&json);
                i32::from(!report.valid)
            }
            Err(error) => {
                eprintln!("could not serialize verification report: {error}");
                1
            }
        };
    }

    match result {
        Ok(report) if report.valid => {
            println_line(&format!(
                "Verified {} artifact(s) in {}.",
                report.artifact_count, report.output
            ));
            0
        }
        Ok(report) => {
            for diagnostic in &report.diagnostics {
                println_line(&format!("{}: {}", diagnostic.id, diagnostic.message));
            }
            println_line(&format!(
                "\n{} verification issue(s) found.",
                report.diagnostics.len()
            ));
            1
        }
        Err(error) => {
            eprintln!("{}", error.message);
            1
        }
    }
}

struct OperationalError {
    id: &'static str,
    message: String,
}

#[derive(serde::Serialize)]
struct VerifyReport {
    schema_version: u32,
    output: String,
    valid: bool,
    artifact_count: usize,
    diagnostics: Vec<VerificationDiagnostic>,
}

impl VerifyReport {
    fn failure(output: String, error: OperationalError) -> Self {
        Self {
            schema_version: crate::run_contract::SCHEMA_VERSION,
            output,
            valid: false,
            artifact_count: 0,
            diagnostics: vec![VerificationDiagnostic {
                id: error.id,
                path: crate::run_contract::RECEIPT_FILE.into(),
                message: error.message,
                expected_digest: None,
                actual_digest: None,
            }],
        }
    }
}

#[derive(serde::Serialize)]
struct VerificationDiagnostic {
    id: &'static str,
    path: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_digest: Option<String>,
}

fn verify_output(output_dir: &Path) -> std::result::Result<VerifyReport, ReceiptReadError> {
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
    Ok(VerifyReport {
        schema_version: crate::run_contract::SCHEMA_VERSION,
        output: output_dir.display().to_string(),
        valid: diagnostics.is_empty(),
        artifact_count: receipt.artifacts.len(),
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
}

impl TrackedKind {
    const fn follows_symlinks(self) -> bool {
        !matches!(self, Self::Artifact)
    }

    const fn mismatch_id(self) -> &'static str {
        match self {
            Self::Source => "CUE-VERIFY-SOURCE-MISMATCH",
            Self::Correction => "CUE-VERIFY-CORRECTION-MISMATCH",
            Self::Artifact => "CUE-VERIFY-ARTIFACT-MISMATCH",
        }
    }

    const fn missing_id(self) -> &'static str {
        match self {
            Self::Source => "CUE-VERIFY-SOURCE-MISSING",
            Self::Correction => "CUE-VERIFY-CORRECTION-MISSING",
            Self::Artifact => "CUE-VERIFY-ARTIFACT-MISSING",
        }
    }

    const fn unsafe_id(self) -> &'static str {
        match self {
            Self::Source => "CUE-VERIFY-SOURCE-UNSAFE",
            Self::Correction => "CUE-VERIFY-CORRECTION-UNSAFE",
            Self::Artifact => "CUE-VERIFY-ARTIFACT-UNSAFE",
        }
    }
}
